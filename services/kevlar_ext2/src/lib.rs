// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Read-write ext2/ext3/ext4 filesystem for Kevlar.
//!
//! Clean-room implementation from the publicly documented ext2/ext4 on-disk format:
//! - "The Second Extended Filesystem" by Dave Poirier
//! - OSDev wiki ext2/ext4 pages
//! - kernel.org ext4 disk layout documentation
//!
//! Full read-write support for ext2 (block pointers) and ext4 (extent trees).
//! On ext4 filesystems, new files use extent trees with contiguous allocation.
//! Extent tree splitting (depth 0→1) is supported for files exceeding 4 extents.
//!
//! This is a Ring 2 service crate — no unsafe code.
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::borrow::Cow;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::cmp::min;
use core::fmt;

use kevlar_api::driver::block::{block_device, BlockDevice};
use kevlar_platform::spinlock::SpinLock;
use kevlar_vfs::{
    file_system::FileSystem,
    inode::{
        DirEntry, Directory, FileLike, FileType, INode, INodeNo, OpenOptions, PollStatus,
        Symlink as SymlinkTrait,
    },
    result::{Errno, Error, Result},
    stat::{
        BlockCount, BlockSize, DevId, FileMode, FileSize, GId, NLink, Stat, Time, UId, S_IFDIR,
        S_IFLNK, S_IFREG,
    },
    user_buffer::{UserBufReader, UserBufWriter, UserBuffer, UserBufferMut},
};

// ext2/ext4 magic number (shared).
const EXT2_SUPER_MAGIC: u16 = 0xEF53;

// Superblock offset and size.
const SUPERBLOCK_OFFSET: u64 = 1024;
const SUPERBLOCK_SIZE: usize = 1024;

// Default block group descriptor size (ext2/ext3).
const GROUP_DESC_SIZE_32: usize = 32;

// ext2 inode mode type bits.
const EXT2_S_IFREG: u16 = 0x8000;
const EXT2_S_IFDIR: u16 = 0x4000;
const EXT2_S_IFLNK: u16 = 0xA000;

// ext2 directory file types.
#[allow(dead_code)]
const EXT2_FT_UNKNOWN: u8 = 0;
const EXT2_FT_REG_FILE: u8 = 1;
const EXT2_FT_DIR: u8 = 2;
const EXT2_FT_SYMLINK: u8 = 7;

// Root inode is always 2 in ext2/ext4.
const EXT2_ROOT_INO: u32 = 2;

// Number of direct block pointers.
const EXT2_NDIR_BLOCKS: usize = 12;
// Indirect block pointer indices.
const EXT2_IND_BLOCK: usize = 12;
const EXT2_DIND_BLOCK: usize = 13;

// ── ext4 feature flags ─────────────────────────────────────────────
const INCOMPAT_FILETYPE: u32 = 0x0002;
const INCOMPAT_RECOVER: u32 = 0x0004;
const INCOMPAT_JOURNAL_DEV: u32 = 0x0008;
const INCOMPAT_EXTENTS: u32 = 0x0040;
const INCOMPAT_64BIT: u32 = 0x0080;
const INCOMPAT_FLEX_BG: u32 = 0x0200;
const INCOMPAT_MMP: u32 = 0x0400;
const INCOMPAT_LARGEDIR: u32 = 0x4000;
const INCOMPAT_CSUM_SEED: u32 = 0x2000;

/// Bitmask of all incompatible features we can safely handle.
const INCOMPAT_SUPPORTED: u32 = INCOMPAT_FILETYPE
    | INCOMPAT_RECOVER
    | INCOMPAT_JOURNAL_DEV
    | INCOMPAT_EXTENTS
    | INCOMPAT_64BIT
    | INCOMPAT_FLEX_BG
    | INCOMPAT_MMP
    | INCOMPAT_LARGEDIR
    | INCOMPAT_CSUM_SEED;

/// Inode flag: this inode uses an extent tree instead of indirect blocks.
const EXT4_EXTENTS_FL: u32 = 0x0008_0000;

/// Compatible feature: filesystem has a journal.
const COMPAT_HAS_JOURNAL: u32 = 0x0004;

/// Extent tree header magic.
const EXT4_EXT_MAGIC: u16 = 0xF30A;

/// Maximum extent tree depth (prevents runaway recursion on corrupt images).
const EXT4_MAX_EXTENT_DEPTH: u16 = 5;

// ── On-disk structures ─────────────────────────────────────────────

/// On-disk ext2/ext4 superblock fields we care about.
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct Ext2Superblock {
    inodes_count: u32,
    blocks_count: u32,
    free_blocks_count: u32,
    free_inodes_count: u32,
    first_data_block: u32,
    log_block_size: u32,
    blocks_per_group: u32,
    inodes_per_group: u32,
    magic: u16,
    first_ino: u32,
    inode_size: u16,
    feature_compat: u32,
    feature_incompat: u32,
    feature_ro_compat: u32,
    desc_size: u16,
}

impl Ext2Superblock {
    fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 256 {
            return None;
        }
        let sb = Ext2Superblock {
            inodes_count: read_u32(data, 0),
            blocks_count: read_u32(data, 4),
            free_blocks_count: read_u32(data, 12),
            free_inodes_count: read_u32(data, 16),
            first_data_block: read_u32(data, 20),
            log_block_size: read_u32(data, 24),
            blocks_per_group: read_u32(data, 32),
            inodes_per_group: read_u32(data, 40),
            magic: read_u16(data, 56),
            first_ino: read_u32(data, 84),
            inode_size: read_u16(data, 88),
            feature_compat: read_u32(data, 92),
            feature_incompat: read_u32(data, 96),
            feature_ro_compat: read_u32(data, 100),
            desc_size: read_u16(data, 254),
        };
        if sb.magic != EXT2_SUPER_MAGIC {
            return None;
        }
        if sb.feature_incompat & !INCOMPAT_SUPPORTED != 0 {
            log::warn!(
                "ext2: unsupported incompat features 0x{:x} (supported: 0x{:x})",
                sb.feature_incompat,
                INCOMPAT_SUPPORTED,
            );
            return None;
        }
        Some(sb)
    }

    fn block_size(&self) -> usize {
        1024 << self.log_block_size
    }

    fn is_64bit(&self) -> bool {
        self.feature_incompat & INCOMPAT_64BIT != 0
    }

    fn has_extents(&self) -> bool {
        self.feature_incompat & INCOMPAT_EXTENTS != 0
    }

    fn label(&self) -> &'static str {
        if self.has_extents() {
            "ext4"
        } else if self.feature_compat & COMPAT_HAS_JOURNAL != 0 {
            "ext3"
        } else {
            "ext2"
        }
    }

    /// Serialize free counts back to a superblock buffer for writeback.
    fn serialize_free_counts(&self, buf: &mut [u8]) {
        write_u32(buf, 12, self.free_blocks_count);
        write_u32(buf, 16, self.free_inodes_count);
    }
}

/// On-disk ext2/ext4 block group descriptor (extended for write support).
#[derive(Debug, Clone)]
struct Ext2GroupDesc {
    block_bitmap: u64,
    inode_bitmap: u64,
    inode_table: u64,
    free_blocks_count: u16,
    free_inodes_count: u16,
    used_dirs_count: u16,
}

impl Ext2GroupDesc {
    fn parse(data: &[u8], is_64bit: bool) -> Self {
        let bb_lo = read_u32(data, 0) as u64;
        let ib_lo = read_u32(data, 4) as u64;
        let it_lo = read_u32(data, 8) as u64;
        let fb = read_u16(data, 12);
        let fi = read_u16(data, 14);
        let ud = read_u16(data, 16);

        let (bb_hi, ib_hi, it_hi) = if is_64bit && data.len() >= 44 {
            (
                (read_u32(data, 32) as u64) & 0xFFFF,
                (read_u32(data, 36) as u64) & 0xFFFF,
                (read_u32(data, 40) as u64) & 0xFFFF,
            )
        } else {
            (0, 0, 0)
        };

        Ext2GroupDesc {
            block_bitmap: bb_lo | (bb_hi << 32),
            inode_bitmap: ib_lo | (ib_hi << 32),
            inode_table: it_lo | (it_hi << 32),
            free_blocks_count: fb,
            free_inodes_count: fi,
            used_dirs_count: ud,
        }
    }

    /// Serialize this group descriptor back to a byte buffer for writeback.
    fn serialize(&self, buf: &mut [u8], is_64bit: bool) {
        write_u32(buf, 0, self.block_bitmap as u32);
        write_u32(buf, 4, self.inode_bitmap as u32);
        write_u32(buf, 8, self.inode_table as u32);
        write_u16(buf, 12, self.free_blocks_count);
        write_u16(buf, 14, self.free_inodes_count);
        write_u16(buf, 16, self.used_dirs_count);
        if is_64bit && buf.len() >= 44 {
            write_u32(buf, 32, (self.block_bitmap >> 32) as u32);
            write_u32(buf, 36, (self.inode_bitmap >> 32) as u32);
            write_u32(buf, 40, (self.inode_table >> 32) as u32);
        }
    }
}

/// On-disk ext2/ext4 inode.
#[derive(Debug, Clone)]
struct Ext2Inode {
    mode: u16,
    uid: u16,
    size: u32,
    atime: u32,
    ctime: u32,
    mtime: u32,
    gid: u16,
    links_count: u16,
    blocks: u32,
    flags: u32,
    block: [u32; 15],
    size_high: u32,
}

impl Ext2Inode {
    fn parse(data: &[u8]) -> Self {
        let mut block = [0u32; 15];
        for (i, b) in block.iter_mut().enumerate() {
            *b = read_u32(data, 40 + i * 4);
        }
        Ext2Inode {
            mode: read_u16(data, 0),
            uid: read_u16(data, 2),
            size: read_u32(data, 4),
            atime: read_u32(data, 8),
            ctime: read_u32(data, 12),
            mtime: read_u32(data, 16),
            gid: read_u16(data, 24),
            links_count: read_u16(data, 26),
            blocks: read_u32(data, 28),
            flags: read_u32(data, 32),
            block,
            size_high: if data.len() > 112 { read_u32(data, 108) } else { 0 },
        }
    }

    fn file_size(&self) -> u64 {
        (self.size as u64) | ((self.size_high as u64) << 32)
    }

    fn set_file_size(&mut self, size: u64) {
        self.size = size as u32;
        self.size_high = (size >> 32) as u32;
    }

    fn uses_extents(&self) -> bool {
        self.flags & EXT4_EXTENTS_FL != 0
    }

    fn is_dir(&self) -> bool {
        (self.mode & 0xF000) == EXT2_S_IFDIR
    }

    #[allow(dead_code)]
    fn is_regular(&self) -> bool {
        (self.mode & 0xF000) == EXT2_S_IFREG
    }

    fn is_symlink(&self) -> bool {
        (self.mode & 0xF000) == EXT2_S_IFLNK
    }

    fn file_mode_bits(&self) -> u32 {
        let type_bits = match self.mode & 0xF000 {
            x if x == EXT2_S_IFDIR => S_IFDIR,
            x if x == EXT2_S_IFLNK => S_IFLNK,
            _ => S_IFREG,
        };
        type_bits | (self.mode as u32 & 0o7777)
    }

    /// Serialize this inode to an on-disk byte buffer.
    fn serialize(&self, buf: &mut [u8]) {
        write_u16(buf, 0, self.mode);
        write_u16(buf, 2, self.uid);
        write_u32(buf, 4, self.size);
        write_u32(buf, 8, self.atime);
        write_u32(buf, 12, self.ctime);
        write_u32(buf, 16, self.mtime);
        // dtime at offset 20 — leave as-is (0 for live inodes)
        write_u16(buf, 24, self.gid);
        write_u16(buf, 26, self.links_count);
        write_u32(buf, 28, self.blocks);
        write_u32(buf, 32, self.flags);
        // osd1 at offset 36 — leave as-is
        for (i, &b) in self.block.iter().enumerate() {
            write_u32(buf, 40 + i * 4, b);
        }
        // generation at 100, file_acl at 104 — leave as-is
        if buf.len() > 112 {
            write_u32(buf, 108, self.size_high);
        }
    }
}

// ── ext4 extent tree structures ────────────────────────────────────

struct ExtentHeader {
    magic: u16,
    entries: u16,
    #[allow(dead_code)]
    max: u16,
    depth: u16,
}

impl ExtentHeader {
    fn parse(data: &[u8]) -> Self {
        ExtentHeader {
            magic: read_u16(data, 0),
            entries: read_u16(data, 2),
            max: read_u16(data, 4),
            depth: read_u16(data, 6),
        }
    }

    fn serialize(&self, buf: &mut [u8]) {
        write_u16(buf, 0, self.magic);
        write_u16(buf, 2, self.entries);
        write_u16(buf, 4, self.max);
        write_u16(buf, 6, self.depth);
    }
}

struct Extent {
    logical_block: u32,
    len: u16,
    start_hi: u16,
    start_lo: u32,
}

impl Extent {
    fn parse(data: &[u8]) -> Self {
        Extent {
            logical_block: read_u32(data, 0),
            len: read_u16(data, 4),
            start_hi: read_u16(data, 6),
            start_lo: read_u32(data, 8),
        }
    }

    fn physical_start(&self) -> u64 {
        ((self.start_hi as u64) << 32) | (self.start_lo as u64)
    }

    fn block_count(&self) -> u32 {
        (self.len & 0x7FFF) as u32
    }

    fn is_uninitialized(&self) -> bool {
        self.len > 0x8000
    }

    fn new(logical_block: u32, len: u16, phys_start: u64) -> Self {
        Extent {
            logical_block,
            len,
            start_hi: (phys_start >> 32) as u16,
            start_lo: phys_start as u32,
        }
    }

    fn serialize(&self, buf: &mut [u8]) {
        write_u32(buf, 0, self.logical_block);
        write_u16(buf, 4, self.len);
        write_u16(buf, 6, self.start_hi);
        write_u32(buf, 8, self.start_lo);
    }
}

struct ExtentIdx {
    logical_block: u32,
    leaf_lo: u32,
    leaf_hi: u16,
}

impl ExtentIdx {
    fn parse(data: &[u8]) -> Self {
        ExtentIdx {
            logical_block: read_u32(data, 0),
            leaf_lo: read_u32(data, 4),
            leaf_hi: read_u16(data, 8),
        }
    }

    fn leaf_block(&self) -> u64 {
        ((self.leaf_hi as u64) << 32) | (self.leaf_lo as u64)
    }

    fn new(logical_block: u32, leaf_block: u64) -> Self {
        ExtentIdx {
            logical_block,
            leaf_lo: leaf_block as u32,
            leaf_hi: (leaf_block >> 32) as u16,
        }
    }

    fn serialize(&self, buf: &mut [u8]) {
        write_u32(buf, 0, self.logical_block);
        write_u32(buf, 4, self.leaf_lo);
        write_u16(buf, 8, self.leaf_hi);
        write_u16(buf, 10, 0); // padding
    }
}

// ── Mutable filesystem state ───────────────────────────────────────

/// Mutable state protected by a SpinLock.
struct Ext2MutableState {
    groups: Vec<Ext2GroupDesc>,
    free_blocks_count: u32,
    free_inodes_count: u32,
}

// ── Shared filesystem inner ────────────────────────────────────────

/// Shared filesystem state. All Ext2Dir/Ext2File/Ext2Symlink instances
/// hold `Arc<Ext2Inner>` so they share mutable state.
struct Ext2Inner {
    device: Arc<dyn BlockDevice>,
    superblock: Ext2Superblock,
    block_size: usize,
    inodes_per_group: u32,
    inode_size: usize,
    is_64bit: bool,
    group_desc_size: usize,
    blocks_per_group: u32,
    first_data_block: u32,
    state: SpinLock<Ext2MutableState>,
    dev_id: usize,
}

impl Ext2Inner {
    // ── Block I/O ──────────────────────────────────────────────────

    fn read_block(&self, block_num: u64) -> Result<Vec<u8>> {
        let sector = block_num * (self.block_size as u64 / 512);
        let mut buf = vec![0u8; self.block_size];
        self.device
            .read_sectors(sector, &mut buf)
            .map_err(|_| Error::new(Errno::EIO))?;
        Ok(buf)
    }

    fn write_block(&self, block_num: u64, data: &[u8]) -> Result<()> {
        let sector = block_num * (self.block_size as u64 / 512);
        self.device
            .write_sectors(sector, data)
            .map_err(|_| Error::new(Errno::EIO))?;
        Ok(())
    }

    // ── Inode I/O ──────────────────────────────────────────────────

    fn read_inode(&self, ino: u32) -> Result<Ext2Inode> {
        let state = self.state.lock_no_irq();
        let group = ((ino - 1) / self.inodes_per_group) as usize;
        let index = ((ino - 1) % self.inodes_per_group) as usize;

        if group >= state.groups.len() {
            return Err(Error::new(Errno::EIO));
        }

        let inode_table_block = state.groups[group].inode_table;
        drop(state);

        let byte_offset = index * self.inode_size;
        let block_offset = byte_offset / self.block_size;
        let offset_in_block = byte_offset % self.block_size;

        let block_data = self.read_block(inode_table_block + block_offset as u64)?;
        Ok(Ext2Inode::parse(&block_data[offset_in_block..]))
    }

    fn write_inode(&self, ino: u32, inode: &Ext2Inode) -> Result<()> {
        let state = self.state.lock_no_irq();
        let group = ((ino - 1) / self.inodes_per_group) as usize;
        let index = ((ino - 1) % self.inodes_per_group) as usize;

        if group >= state.groups.len() {
            return Err(Error::new(Errno::EIO));
        }

        let inode_table_block = state.groups[group].inode_table;
        drop(state);

        let byte_offset = index * self.inode_size;
        let block_offset = byte_offset / self.block_size;
        let offset_in_block = byte_offset % self.block_size;

        let mut block_data = self.read_block(inode_table_block + block_offset as u64)?;
        inode.serialize(&mut block_data[offset_in_block..]);
        self.write_block(inode_table_block + block_offset as u64, &block_data)
    }

    // ── File data reading ──────────────────────────────────────────

    fn read_file_data(&self, inode: &Ext2Inode, offset: usize, len: usize) -> Result<Vec<u8>> {
        let file_size = inode.file_size() as usize;
        if offset >= file_size {
            return Ok(Vec::new());
        }

        let read_len = min(len, file_size - offset);
        let mut result = Vec::with_capacity(read_len);
        let mut remaining = read_len;
        let mut pos = offset;

        let ptrs_per_block = self.block_size / 4;
        let use_extents = inode.uses_extents();

        while remaining > 0 {
            let block_index = pos / self.block_size;
            let offset_in_block = pos % self.block_size;
            let chunk_len = min(remaining, self.block_size - offset_in_block);

            let block_num = if use_extents {
                self.resolve_extent(inode, block_index)?
            } else {
                self.resolve_block_ptr(inode, block_index, ptrs_per_block)? as u64
            };

            if block_num == 0 {
                result.extend(core::iter::repeat(0u8).take(chunk_len));
            } else {
                let block_data = self.read_block(block_num)?;
                result.extend_from_slice(&block_data[offset_in_block..offset_in_block + chunk_len]);
            }

            pos += chunk_len;
            remaining -= chunk_len;
        }

        Ok(result)
    }

    fn resolve_block_ptr(
        &self,
        inode: &Ext2Inode,
        block_index: usize,
        ptrs_per_block: usize,
    ) -> Result<u32> {
        if block_index < EXT2_NDIR_BLOCKS {
            return Ok(inode.block[block_index]);
        }

        let index = block_index - EXT2_NDIR_BLOCKS;
        if index < ptrs_per_block {
            let ind_block = inode.block[EXT2_IND_BLOCK];
            if ind_block == 0 {
                return Ok(0);
            }
            let data = self.read_block(ind_block as u64)?;
            return Ok(read_u32(&data, index * 4));
        }

        let index = index - ptrs_per_block;
        if index < ptrs_per_block * ptrs_per_block {
            let dind_block = inode.block[EXT2_DIND_BLOCK];
            if dind_block == 0 {
                return Ok(0);
            }
            let l1_data = self.read_block(dind_block as u64)?;
            let l1_index = index / ptrs_per_block;
            let l2_index = index % ptrs_per_block;
            let l2_block = read_u32(&l1_data, l1_index * 4);
            if l2_block == 0 {
                return Ok(0);
            }
            let l2_data = self.read_block(l2_block as u64)?;
            return Ok(read_u32(&l2_data, l2_index * 4));
        }

        Err(Error::new(Errno::EFBIG))
    }

    // ── ext4 extent tree resolution ────────────────────────────────

    fn resolve_extent(&self, inode: &Ext2Inode, logical_block: usize) -> Result<u64> {
        let mut root_data = [0u8; 60];
        for (i, &b) in inode.block.iter().enumerate() {
            let bytes = b.to_le_bytes();
            root_data[i * 4..i * 4 + 4].copy_from_slice(&bytes);
        }

        self.resolve_extent_in_node(&root_data, logical_block as u32, EXT4_MAX_EXTENT_DEPTH)
    }

    fn resolve_extent_in_node(
        &self,
        node_data: &[u8],
        logical_block: u32,
        depth_limit: u16,
    ) -> Result<u64> {
        if node_data.len() < 12 {
            return Err(Error::new(Errno::EIO));
        }

        let header = ExtentHeader::parse(node_data);
        if header.magic != EXT4_EXT_MAGIC {
            return Err(Error::new(Errno::EIO));
        }
        if header.depth > depth_limit {
            return Err(Error::new(Errno::EIO));
        }

        if header.depth == 0 {
            for i in 0..header.entries as usize {
                let off = 12 + i * 12;
                if off + 12 > node_data.len() {
                    break;
                }
                let ext = Extent::parse(&node_data[off..]);
                let start = ext.logical_block;
                let end = start + ext.block_count();
                if logical_block >= start && logical_block < end {
                    if ext.is_uninitialized() {
                        return Ok(0);
                    }
                    let offset_within = (logical_block - start) as u64;
                    return Ok(ext.physical_start() + offset_within);
                }
            }
            Ok(0)
        } else {
            let mut child_block: Option<u64> = None;
            for i in 0..header.entries as usize {
                let off = 12 + i * 12;
                if off + 12 > node_data.len() {
                    break;
                }
                let idx = ExtentIdx::parse(&node_data[off..]);
                if idx.logical_block <= logical_block {
                    child_block = Some(idx.leaf_block());
                } else {
                    break;
                }
            }

            let child = child_block.ok_or_else(|| Error::new(Errno::EIO))?;
            let child_data = self.read_block(child)?;
            self.resolve_extent_in_node(&child_data, logical_block, depth_limit - 1)
        }
    }

    // ── Extent tree helpers ──────────────────────────────────────

    /// Read the inode's i_block[15] as a 60-byte extent tree root buffer.
    fn extent_root_data(inode: &Ext2Inode) -> [u8; 60] {
        let mut data = [0u8; 60];
        for (i, &b) in inode.block.iter().enumerate() {
            let bytes = b.to_le_bytes();
            data[i * 4..i * 4 + 4].copy_from_slice(&bytes);
        }
        data
    }

    /// Write a 60-byte extent tree root buffer back into the inode's i_block[15].
    fn set_extent_root_data(inode: &mut Ext2Inode, data: &[u8; 60]) {
        for i in 0..15 {
            inode.block[i] = u32::from_le_bytes([
                data[i * 4],
                data[i * 4 + 1],
                data[i * 4 + 2],
                data[i * 4 + 3],
            ]);
        }
    }

    /// Initialize an empty extent tree root (depth=0, entries=0, max=4).
    fn init_extent_root() -> [u32; 15] {
        let mut data = [0u8; 60];
        let header = ExtentHeader {
            magic: EXT4_EXT_MAGIC,
            entries: 0,
            max: 4,
            depth: 0,
        };
        header.serialize(&mut data);
        let mut block = [0u32; 15];
        for i in 0..15 {
            block[i] = u32::from_le_bytes([
                data[i * 4],
                data[i * 4 + 1],
                data[i * 4 + 2],
                data[i * 4 + 3],
            ]);
        }
        block
    }

    /// Allocate a block for an extent-based file at `logical_block`.
    /// Tries to extend an adjacent extent for contiguity; otherwise inserts
    /// a new extent entry. Returns the allocated physical block number.
    fn alloc_extent_block(
        &self,
        inode: &mut Ext2Inode,
        ino: u32,
        logical_block: usize,
    ) -> Result<u64> {
        let mut root = Self::extent_root_data(inode);
        let header = ExtentHeader::parse(&root);
        if header.magic != EXT4_EXT_MAGIC {
            return Err(Error::new(Errno::EIO));
        }

        if header.depth == 0 {
            // Leaf-only tree (common case): insert directly in root.
            let result = self.insert_extent_in_leaf(
                &mut root, header.max, logical_block as u32,
            )?;
            Self::set_extent_root_data(inode, &root);
            inode.blocks += (self.block_size / 512) as u32;
            self.write_inode(ino, inode)?;
            Ok(result)
        } else {
            // Multi-level tree: traverse to the correct leaf.
            let leaf_block = self.find_extent_leaf(&root, logical_block as u32, header.depth)?;
            let mut leaf_data = self.read_block(leaf_block)?;
            let leaf_header = ExtentHeader::parse(&leaf_data);
            let max = leaf_header.max;

            let result = self.insert_extent_in_leaf(
                &mut leaf_data, max, logical_block as u32,
            )?;
            self.write_block(leaf_block, &leaf_data)?;
            inode.blocks += (self.block_size / 512) as u32;
            self.write_inode(ino, inode)?;
            Ok(result)
        }
    }

    /// Find the leaf block that should contain `logical_block` in a multi-level tree.
    fn find_extent_leaf(&self, root: &[u8], logical_block: u32, depth: u16) -> Result<u64> {
        let mut node_data: Vec<u8> = root.to_vec();
        let mut current_depth = depth;

        while current_depth > 0 {
            let header = ExtentHeader::parse(&node_data);
            let mut child_block: Option<u64> = None;

            for i in 0..header.entries as usize {
                let off = 12 + i * 12;
                if off + 12 > node_data.len() {
                    break;
                }
                let idx = ExtentIdx::parse(&node_data[off..]);
                if idx.logical_block <= logical_block {
                    child_block = Some(idx.leaf_block());
                } else {
                    break;
                }
            }

            // If no child found (logical_block < first index), use the first child.
            let child = child_block.or_else(|| {
                if header.entries > 0 {
                    Some(ExtentIdx::parse(&node_data[12..]).leaf_block())
                } else {
                    None
                }
            }).ok_or_else(|| Error::new(Errno::EIO))?;

            if current_depth == 1 {
                return Ok(child);
            }

            node_data = self.read_block(child)?;
            current_depth -= 1;
        }

        Err(Error::new(Errno::EIO))
    }

    /// Insert a mapping for `logical_block` into a leaf node.
    /// The leaf can be either the root (60 bytes) or a disk block (block_size bytes).
    /// Returns the allocated physical block number.
    fn insert_extent_in_leaf(
        &self,
        leaf: &mut [u8],
        max_entries: u16,
        logical_block: u32,
    ) -> Result<u64> {
        let mut header = ExtentHeader::parse(leaf);
        if header.magic != EXT4_EXT_MAGIC {
            return Err(Error::new(Errno::EIO));
        }

        let entries = header.entries as usize;

        // Try to extend an existing adjacent extent.
        for i in 0..entries {
            let off = 12 + i * 12;
            let ext = Extent::parse(&leaf[off..]);
            let ext_end = ext.logical_block + ext.block_count();

            // Append: new block is right after this extent.
            if ext_end == logical_block && ext.block_count() < 32768 && !ext.is_uninitialized() {
                let goal = ext.physical_start() + ext.block_count() as u64;
                let new_phys = self.alloc_block_near(goal)?;
                if new_phys == goal {
                    // Contiguous! Extend the extent.
                    let new_len = ext.len + 1;
                    write_u16(leaf, off + 4, new_len);
                    return Ok(new_phys);
                }
                // Not contiguous — free and fall through to insert.
                self.free_block(new_phys)?;
            }

            // Prepend: new block is right before this extent.
            if ext.logical_block == logical_block + 1 && ext.block_count() < 32768
                && !ext.is_uninitialized() && ext.physical_start() > 0
            {
                let goal = ext.physical_start() - 1;
                let new_phys = self.alloc_block_near(goal)?;
                if new_phys == goal {
                    // Contiguous prepend.
                    let new_ext = Extent::new(
                        logical_block,
                        ext.len + 1,
                        new_phys,
                    );
                    new_ext.serialize(&mut leaf[off..]);
                    return Ok(new_phys);
                }
                self.free_block(new_phys)?;
            }
        }

        // Can't extend — need to insert a new single-block extent.
        if entries >= max_entries as usize {
            // Leaf is full — need to split. For now, try root split (depth 0 → 1).
            // This handles the most common case (root with 4 entries).
            return self.split_and_insert(leaf, max_entries, logical_block);
        }

        // Allocate a block.
        let new_phys = self.alloc_block()?;
        let new_ext = Extent::new(logical_block, 1, new_phys);

        // Find insertion position (entries are sorted by logical_block).
        let mut insert_pos = entries;
        for i in 0..entries {
            let off = 12 + i * 12;
            let ext = Extent::parse(&leaf[off..]);
            if ext.logical_block > logical_block {
                insert_pos = i;
                break;
            }
        }

        // Shift entries right to make room.
        for i in (insert_pos..entries).rev() {
            let src_off = 12 + i * 12;
            let dst_off = 12 + (i + 1) * 12;
            // Copy 12 bytes from src to dst.
            let mut tmp = [0u8; 12];
            tmp.copy_from_slice(&leaf[src_off..src_off + 12]);
            leaf[dst_off..dst_off + 12].copy_from_slice(&tmp);
        }

        // Write the new extent.
        new_ext.serialize(&mut leaf[12 + insert_pos * 12..]);

        // Update header.
        header.entries += 1;
        header.serialize(leaf);

        Ok(new_phys)
    }

    /// Split a full leaf (root or disk block) and insert a new extent.
    /// For depth-0 root: promotes to depth 1 with two leaf children.
    fn split_and_insert(
        &self,
        root: &mut [u8],
        max_entries: u16,
        logical_block: u32,
    ) -> Result<u64> {
        let header = ExtentHeader::parse(root);
        let entries = header.entries as usize;

        if header.depth == 0 && root.len() <= 60 {
            // Root leaf split: depth 0 → 1.
            // Collect all existing extents.
            let mut extents: Vec<Extent> = Vec::with_capacity(entries);
            for i in 0..entries {
                extents.push(Extent::parse(&root[12 + i * 12..]));
            }

            // Allocate the new physical block for the new extent.
            let new_phys = self.alloc_block()?;
            let new_ext = Extent::new(logical_block, 1, new_phys);

            // Insert into sorted position.
            let mut insert_pos = extents.len();
            for (i, ext) in extents.iter().enumerate() {
                if ext.logical_block > logical_block {
                    insert_pos = i;
                    break;
                }
            }
            extents.insert(insert_pos, new_ext);

            // Split: lower half → leaf A, upper half → leaf B.
            let mid = extents.len() / 2;
            let leaf_a_extents = &extents[..mid];
            let leaf_b_extents = &extents[mid..];

            // Allocate two disk blocks for the new leaves.
            let leaf_a_block = self.alloc_block()?;
            let leaf_b_block = self.alloc_block()?;

            let max_per_leaf = ((self.block_size - 12) / 12) as u16;

            // Write leaf A.
            let mut leaf_a = vec![0u8; self.block_size];
            let hdr_a = ExtentHeader {
                magic: EXT4_EXT_MAGIC,
                entries: leaf_a_extents.len() as u16,
                max: max_per_leaf,
                depth: 0,
            };
            hdr_a.serialize(&mut leaf_a);
            for (i, ext) in leaf_a_extents.iter().enumerate() {
                ext.serialize(&mut leaf_a[12 + i * 12..]);
            }
            self.write_block(leaf_a_block, &leaf_a)?;

            // Write leaf B.
            let mut leaf_b = vec![0u8; self.block_size];
            let hdr_b = ExtentHeader {
                magic: EXT4_EXT_MAGIC,
                entries: leaf_b_extents.len() as u16,
                max: max_per_leaf,
                depth: 0,
            };
            hdr_b.serialize(&mut leaf_b);
            for (i, ext) in leaf_b_extents.iter().enumerate() {
                ext.serialize(&mut leaf_b[12 + i * 12..]);
            }
            self.write_block(leaf_b_block, &leaf_b)?;

            // Rewrite root as depth-1 with 2 ExtentIdx entries.
            // Root max stays at 4 (same space, now holds ExtentIdx instead of Extent).
            let new_root_hdr = ExtentHeader {
                magic: EXT4_EXT_MAGIC,
                entries: 2,
                max: max_entries,
                depth: 1,
            };
            // Zero out root first.
            let root_clear_len = 60.min(root.len());
            for b in root[..root_clear_len].iter_mut() {
                *b = 0;
            }
            new_root_hdr.serialize(root);

            let idx_a = ExtentIdx::new(leaf_a_extents[0].logical_block, leaf_a_block);
            idx_a.serialize(&mut root[12..]);

            let idx_b = ExtentIdx::new(leaf_b_extents[0].logical_block, leaf_b_block);
            idx_b.serialize(&mut root[24..]);

            return Ok(new_phys);
        }

        // Disk-block leaf split or internal node split — allocate new block,
        // move upper half. For now, return ENOSPC if we can't handle it.
        // This path is extremely unlikely: a single disk leaf holds 340 entries,
        // and with contiguous allocation each extent covers many blocks.
        Err(Error::new(Errno::ENOSPC))
    }

    /// Free all data blocks in an extent tree (recursive).
    fn free_extent_blocks(&self, inode: &Ext2Inode) -> Result<()> {
        let root = Self::extent_root_data(inode);
        self.free_extent_node(&root)
    }

    fn free_extent_node(&self, node_data: &[u8]) -> Result<()> {
        let header = ExtentHeader::parse(node_data);
        if header.magic != EXT4_EXT_MAGIC {
            return Err(Error::new(Errno::EIO));
        }

        if header.depth == 0 {
            // Leaf: free each extent's physical block range.
            for i in 0..header.entries as usize {
                let off = 12 + i * 12;
                if off + 12 > node_data.len() {
                    break;
                }
                let ext = Extent::parse(&node_data[off..]);
                if ext.is_uninitialized() {
                    continue;
                }
                for offset in 0..ext.block_count() as u64 {
                    self.free_block(ext.physical_start() + offset)?;
                }
            }
        } else {
            // Internal: recurse into children, then free child blocks.
            for i in 0..header.entries as usize {
                let off = 12 + i * 12;
                if off + 12 > node_data.len() {
                    break;
                }
                let idx = ExtentIdx::parse(&node_data[off..]);
                let child_data = self.read_block(idx.leaf_block())?;
                self.free_extent_node(&child_data)?;
                self.free_block(idx.leaf_block())?;
            }
        }

        Ok(())
    }

    // ── Block/Inode bitmap allocation ──────────────────────────────

    /// Allocate a free block near `goal` for extent contiguity.
    /// Tries the goal's group first, starting from the goal bit, then other groups.
    fn alloc_block_near(&self, goal: u64) -> Result<u64> {
        let mut state = self.state.lock_no_irq();
        if state.free_blocks_count == 0 {
            return Err(Error::new(Errno::ENOSPC));
        }

        let goal_relative = goal.saturating_sub(self.first_data_block as u64);
        let goal_group = (goal_relative / self.blocks_per_group as u64) as usize;
        let goal_bit = (goal_relative % self.blocks_per_group as u64) as usize;
        let num_groups = state.groups.len();

        // Try goal group first, then wrap around.
        for offset in 0..num_groups {
            let group_idx = (goal_group + offset) % num_groups;
            if state.groups[group_idx].free_blocks_count == 0 {
                continue;
            }

            let bitmap_block = state.groups[group_idx].block_bitmap;
            drop(state);

            let mut bitmap = self.read_block(bitmap_block)?;

            state = self.state.lock_no_irq();
            if state.groups[group_idx].free_blocks_count == 0 {
                continue;
            }

            let max_bits = self.blocks_per_group as usize;
            // Start from goal_bit in the goal group, from 0 otherwise.
            let start_bit = if group_idx == goal_group { goal_bit } else { 0 };

            if let Some(bit) = find_free_bit_from(&bitmap, start_bit, max_bits) {
                bitmap[bit / 8] |= 1 << (bit % 8);
                state.groups[group_idx].free_blocks_count -= 1;
                state.free_blocks_count -= 1;

                let block_num = group_idx as u64 * self.blocks_per_group as u64
                    + self.first_data_block as u64
                    + bit as u64;

                let groups_clone = state.groups.clone();
                let free_blocks = state.free_blocks_count;
                let free_inodes = state.free_inodes_count;
                drop(state);

                self.write_block(bitmap_block, &bitmap)?;
                self.flush_metadata(&groups_clone, free_blocks, free_inodes)?;

                return Ok(block_num);
            }
        }

        Err(Error::new(Errno::ENOSPC))
    }

    /// Allocate a free block. Returns the block number.
    fn alloc_block(&self) -> Result<u64> {
        let mut state = self.state.lock_no_irq();
        if state.free_blocks_count == 0 {
            return Err(Error::new(Errno::ENOSPC));
        }

        for group_idx in 0..state.groups.len() {
            if state.groups[group_idx].free_blocks_count == 0 {
                continue;
            }

            let bitmap_block = state.groups[group_idx].block_bitmap;
            // Drop lock during I/O, re-acquire after.
            drop(state);

            let mut bitmap = self.read_block(bitmap_block)?;

            state = self.state.lock_no_irq();
            // Re-check after re-acquiring lock.
            if state.groups[group_idx].free_blocks_count == 0 {
                continue;
            }

            // Scan bitmap for first free bit.
            let max_bits = self.blocks_per_group as usize;
            if let Some(bit) = find_free_bit(&bitmap, max_bits) {
                // Set the bit.
                bitmap[bit / 8] |= 1 << (bit % 8);

                // Update counts.
                state.groups[group_idx].free_blocks_count -= 1;
                state.free_blocks_count -= 1;

                let block_num =
                    group_idx as u64 * self.blocks_per_group as u64
                    + self.first_data_block as u64
                    + bit as u64;

                // Drop lock during I/O.
                let groups_clone = state.groups.clone();
                let free_blocks = state.free_blocks_count;
                let free_inodes = state.free_inodes_count;
                drop(state);

                self.write_block(bitmap_block, &bitmap)?;
                self.flush_metadata(&groups_clone, free_blocks, free_inodes)?;

                return Ok(block_num);
            }

            // Bitmap says free but we couldn't find one — try next group.
        }

        Err(Error::new(Errno::ENOSPC))
    }

    /// Free a previously allocated block.
    fn free_block(&self, block_num: u64) -> Result<()> {
        let relative = block_num - self.first_data_block as u64;
        let group_idx = (relative / self.blocks_per_group as u64) as usize;
        let bit = (relative % self.blocks_per_group as u64) as usize;

        let state = self.state.lock_no_irq();
        if group_idx >= state.groups.len() {
            return Err(Error::new(Errno::EIO));
        }

        let bitmap_block = state.groups[group_idx].block_bitmap;
        drop(state);

        let mut bitmap = self.read_block(bitmap_block)?;
        bitmap[bit / 8] &= !(1 << (bit % 8));

        let mut state = self.state.lock_no_irq();
        state.groups[group_idx].free_blocks_count += 1;
        state.free_blocks_count += 1;

        let groups_clone = state.groups.clone();
        let free_blocks = state.free_blocks_count;
        let free_inodes = state.free_inodes_count;
        drop(state);

        self.write_block(bitmap_block, &bitmap)?;
        self.flush_metadata(&groups_clone, free_blocks, free_inodes)?;
        Ok(())
    }

    /// Allocate a free inode. Returns the inode number. Zeros the on-disk inode.
    fn alloc_inode(&self, is_dir: bool) -> Result<u32> {
        let mut state = self.state.lock_no_irq();
        if state.free_inodes_count == 0 {
            return Err(Error::new(Errno::ENOSPC));
        }

        for group_idx in 0..state.groups.len() {
            if state.groups[group_idx].free_inodes_count == 0 {
                continue;
            }

            let bitmap_block = state.groups[group_idx].inode_bitmap;
            drop(state);

            let mut bitmap = self.read_block(bitmap_block)?;

            state = self.state.lock_no_irq();
            if state.groups[group_idx].free_inodes_count == 0 {
                continue;
            }

            let max_bits = self.inodes_per_group as usize;
            if let Some(bit) = find_free_bit(&bitmap, max_bits) {
                bitmap[bit / 8] |= 1 << (bit % 8);

                state.groups[group_idx].free_inodes_count -= 1;
                state.free_inodes_count -= 1;
                if is_dir {
                    state.groups[group_idx].used_dirs_count += 1;
                }

                let ino = group_idx as u32 * self.inodes_per_group + bit as u32 + 1;

                let groups_clone = state.groups.clone();
                let free_blocks = state.free_blocks_count;
                let free_inodes = state.free_inodes_count;
                drop(state);

                self.write_block(bitmap_block, &bitmap)?;
                self.flush_metadata(&groups_clone, free_blocks, free_inodes)?;

                // Zero the on-disk inode.
                let zero_inode = Ext2Inode {
                    mode: 0,
                    uid: 0,
                    size: 0,
                    atime: 0,
                    ctime: 0,
                    mtime: 0,
                    gid: 0,
                    links_count: 0,
                    blocks: 0,
                    flags: 0,
                    block: [0; 15],
                    size_high: 0,
                };
                self.write_inode(ino, &zero_inode)?;

                return Ok(ino);
            }
        }

        Err(Error::new(Errno::ENOSPC))
    }

    /// Free a previously allocated inode.
    fn free_inode(&self, ino: u32, is_dir: bool) -> Result<()> {
        let group_idx = ((ino - 1) / self.inodes_per_group) as usize;
        let bit = ((ino - 1) % self.inodes_per_group) as usize;

        let state = self.state.lock_no_irq();
        if group_idx >= state.groups.len() {
            return Err(Error::new(Errno::EIO));
        }

        let bitmap_block = state.groups[group_idx].inode_bitmap;
        drop(state);

        let mut bitmap = self.read_block(bitmap_block)?;
        bitmap[bit / 8] &= !(1 << (bit % 8));

        let mut state = self.state.lock_no_irq();
        state.groups[group_idx].free_inodes_count += 1;
        state.free_inodes_count += 1;
        if is_dir {
            state.groups[group_idx].used_dirs_count =
                state.groups[group_idx].used_dirs_count.saturating_sub(1);
        }

        let groups_clone = state.groups.clone();
        let free_blocks = state.free_blocks_count;
        let free_inodes = state.free_inodes_count;
        drop(state);

        self.write_block(bitmap_block, &bitmap)?;
        self.flush_metadata(&groups_clone, free_blocks, free_inodes)?;
        Ok(())
    }

    // ── Metadata flush ─────────────────────────────────────────────

    /// Write superblock free counts + all group descriptors to disk.
    fn flush_metadata(
        &self,
        groups: &[Ext2GroupDesc],
        free_blocks: u32,
        free_inodes: u32,
    ) -> Result<()> {
        // Update superblock free counts.
        let sb_sector = SUPERBLOCK_OFFSET / 512;
        let sb_sectors = (SUPERBLOCK_SIZE + 511) / 512;
        let mut sb_buf = vec![0u8; sb_sectors * 512];
        self.device
            .read_sectors(sb_sector, &mut sb_buf)
            .map_err(|_| Error::new(Errno::EIO))?;

        let mut sb_copy = self.superblock.clone();
        sb_copy.free_blocks_count = free_blocks;
        sb_copy.free_inodes_count = free_inodes;
        sb_copy.serialize_free_counts(&mut sb_buf);

        self.device
            .write_sectors(sb_sector, &sb_buf)
            .map_err(|_| Error::new(Errno::EIO))?;

        // Write group descriptor table.
        let gdt_block = if self.block_size == 1024 { 2 } else { 1 };
        let gdt_bytes = groups.len() * self.group_desc_size;
        let gdt_buf_size = ((gdt_bytes + self.block_size - 1) / self.block_size) * self.block_size;
        let mut gdt_buf = vec![0u8; gdt_buf_size];

        // Read existing GDT to preserve fields we don't modify.
        let gdt_sector = gdt_block as u64 * (self.block_size as u64 / 512);
        let gdt_read_sectors = (gdt_buf_size + 511) / 512;
        let mut gdt_read_buf = vec![0u8; gdt_read_sectors * 512];
        self.device
            .read_sectors(gdt_sector, &mut gdt_read_buf)
            .map_err(|_| Error::new(Errno::EIO))?;
        let copy_len = min(gdt_buf.len(), gdt_read_buf.len());
        gdt_buf[..copy_len].copy_from_slice(&gdt_read_buf[..copy_len]);

        for (i, g) in groups.iter().enumerate() {
            let offset = i * self.group_desc_size;
            g.serialize(&mut gdt_buf[offset..], self.is_64bit);
        }

        // Write back in block-sized chunks.
        let blocks_to_write = (gdt_bytes + self.block_size - 1) / self.block_size;
        for b in 0..blocks_to_write {
            let blk = gdt_block as u64 + b as u64;
            let off = b * self.block_size;
            let end = min(off + self.block_size, gdt_buf.len());
            // Pad to full block if needed.
            let mut full_block = vec![0u8; self.block_size];
            full_block[..end - off].copy_from_slice(&gdt_buf[off..end]);
            self.write_block(blk, &full_block)?;
        }

        Ok(())
    }

    // ── Block pointer management ───────────────────────────────────

    /// Set block pointer at `block_index` in the inode to `block_num`.
    /// Allocates indirect/double-indirect blocks on demand.
    fn set_block_ptr(
        &self,
        inode: &mut Ext2Inode,
        block_index: usize,
        block_num: u32,
    ) -> Result<()> {
        let ptrs_per_block = self.block_size / 4;

        if block_index < EXT2_NDIR_BLOCKS {
            inode.block[block_index] = block_num;
            return Ok(());
        }

        let index = block_index - EXT2_NDIR_BLOCKS;
        if index < ptrs_per_block {
            // Single indirect.
            if inode.block[EXT2_IND_BLOCK] == 0 {
                let ind = self.alloc_block()? as u32;
                let zero_block = vec![0u8; self.block_size];
                self.write_block(ind as u64, &zero_block)?;
                inode.block[EXT2_IND_BLOCK] = ind;
            }
            let mut data = self.read_block(inode.block[EXT2_IND_BLOCK] as u64)?;
            write_u32(&mut data, index * 4, block_num);
            self.write_block(inode.block[EXT2_IND_BLOCK] as u64, &data)?;
            return Ok(());
        }

        let index = index - ptrs_per_block;
        if index < ptrs_per_block * ptrs_per_block {
            // Double indirect.
            if inode.block[EXT2_DIND_BLOCK] == 0 {
                let dind = self.alloc_block()? as u32;
                let zero_block = vec![0u8; self.block_size];
                self.write_block(dind as u64, &zero_block)?;
                inode.block[EXT2_DIND_BLOCK] = dind;
            }

            let l1_index = index / ptrs_per_block;
            let l2_index = index % ptrs_per_block;

            let mut l1_data = self.read_block(inode.block[EXT2_DIND_BLOCK] as u64)?;
            let mut l2_block = read_u32(&l1_data, l1_index * 4);

            if l2_block == 0 {
                l2_block = self.alloc_block()? as u32;
                let zero_block = vec![0u8; self.block_size];
                self.write_block(l2_block as u64, &zero_block)?;
                write_u32(&mut l1_data, l1_index * 4, l2_block);
                self.write_block(inode.block[EXT2_DIND_BLOCK] as u64, &l1_data)?;
            }

            let mut l2_data = self.read_block(l2_block as u64)?;
            write_u32(&mut l2_data, l2_index * 4, block_num);
            self.write_block(l2_block as u64, &l2_data)?;
            return Ok(());
        }

        Err(Error::new(Errno::EFBIG))
    }

    /// Allocate a data block for the given block index and update the inode.
    fn alloc_data_block(&self, inode: &mut Ext2Inode, block_index: usize) -> Result<u64> {
        let block_num = self.alloc_block()?;
        self.set_block_ptr(inode, block_index, block_num as u32)?;
        // Update i_blocks (in 512-byte units).
        inode.blocks += (self.block_size / 512) as u32;
        Ok(block_num)
    }

    /// Free all data blocks (and indirect blocks) owned by an inode.
    fn free_file_blocks(&self, inode: &Ext2Inode) -> Result<()> {
        if inode.uses_extents() {
            return self.free_extent_blocks(inode);
        }

        let ptrs_per_block = self.block_size / 4;

        // Free direct blocks.
        for i in 0..EXT2_NDIR_BLOCKS {
            if inode.block[i] != 0 {
                self.free_block(inode.block[i] as u64)?;
            }
        }

        // Free single indirect.
        if inode.block[EXT2_IND_BLOCK] != 0 {
            let data = self.read_block(inode.block[EXT2_IND_BLOCK] as u64)?;
            for i in 0..ptrs_per_block {
                let b = read_u32(&data, i * 4);
                if b != 0 {
                    self.free_block(b as u64)?;
                }
            }
            self.free_block(inode.block[EXT2_IND_BLOCK] as u64)?;
        }

        // Free double indirect.
        if inode.block[EXT2_DIND_BLOCK] != 0 {
            let l1_data = self.read_block(inode.block[EXT2_DIND_BLOCK] as u64)?;
            for i in 0..ptrs_per_block {
                let l2_block = read_u32(&l1_data, i * 4);
                if l2_block != 0 {
                    let l2_data = self.read_block(l2_block as u64)?;
                    for j in 0..ptrs_per_block {
                        let b = read_u32(&l2_data, j * 4);
                        if b != 0 {
                            self.free_block(b as u64)?;
                        }
                    }
                    self.free_block(l2_block as u64)?;
                }
            }
            self.free_block(inode.block[EXT2_DIND_BLOCK] as u64)?;
        }

        Ok(())
    }

    // ── Directory entry helpers ────────────────────────────────────

    fn read_dir_entries(&self, inode: &Ext2Inode) -> Result<Vec<Ext2DirEntryInfo>> {
        let data = self.read_file_data(inode, 0, inode.file_size() as usize)?;
        let mut entries = Vec::new();
        let mut pos = 0;

        while pos + 8 <= data.len() {
            let ino = read_u32(&data, pos);
            let rec_len = read_u16(&data, pos + 4) as usize;
            let name_len = data[pos + 6] as usize;
            let file_type = data[pos + 7];

            if rec_len == 0 {
                break;
            }
            if ino != 0 && name_len > 0 && pos + 8 + name_len <= data.len() {
                let name = core::str::from_utf8(&data[pos + 8..pos + 8 + name_len])
                    .unwrap_or("")
                    .to_string();
                entries.push(Ext2DirEntryInfo {
                    inode: ino,
                    file_type,
                    name,
                });
            }

            pos += rec_len;
        }

        Ok(entries)
    }

    /// Add a directory entry to a directory inode.
    /// Finds space by splitting an existing entry's rec_len, or allocates a new block.
    fn add_dir_entry(
        &self,
        dir_ino: u32,
        dir_inode: &mut Ext2Inode,
        child_ino: u32,
        name: &str,
        file_type: u8,
    ) -> Result<()> {
        let needed = dir_entry_len(name.len());
        let dir_size = dir_inode.file_size() as usize;

        // Walk existing blocks looking for space.
        let ptrs_per_block = self.block_size / 4;
        let num_blocks = (dir_size + self.block_size - 1) / self.block_size;
        let use_extents = dir_inode.uses_extents();

        for blk_idx in 0..num_blocks {
            let block_num = if use_extents {
                self.resolve_extent(dir_inode, blk_idx)?
            } else {
                self.resolve_block_ptr(dir_inode, blk_idx, ptrs_per_block)? as u64
            };
            if block_num == 0 {
                continue;
            }

            let mut data = self.read_block(block_num)?;
            if let Some(()) = try_insert_dir_entry(&mut data, child_ino, name, file_type, needed) {
                self.write_block(block_num, &data)?;
                return Ok(());
            }
        }

        // No space in existing blocks — allocate a new one.
        let new_block = if dir_inode.uses_extents() {
            self.alloc_extent_block(dir_inode, dir_ino, num_blocks)?
        } else {
            self.alloc_data_block(dir_inode, num_blocks)?
        };
        let mut data = vec![0u8; self.block_size];

        // Write the new entry spanning the entire block.
        write_u32(&mut data, 0, child_ino);
        write_u16(&mut data, 4, self.block_size as u16); // rec_len = entire block
        data[6] = name.len() as u8;
        data[7] = file_type;
        data[8..8 + name.len()].copy_from_slice(name.as_bytes());

        self.write_block(new_block, &data)?;

        // Update directory size.
        let new_size = dir_inode.file_size() + self.block_size as u64;
        dir_inode.set_file_size(new_size);
        self.write_inode(dir_ino, dir_inode)?;

        Ok(())
    }

    /// Remove a directory entry by name. Merges rec_len with predecessor.
    fn remove_dir_entry(&self, dir_inode: &Ext2Inode, name: &str) -> Result<()> {
        let dir_size = dir_inode.file_size() as usize;
        let ptrs_per_block = self.block_size / 4;
        let num_blocks = (dir_size + self.block_size - 1) / self.block_size;
        let use_extents = dir_inode.uses_extents();

        for blk_idx in 0..num_blocks {
            let block_num = if use_extents {
                self.resolve_extent(dir_inode, blk_idx)?
            } else {
                self.resolve_block_ptr(dir_inode, blk_idx, ptrs_per_block)? as u64
            };
            if block_num == 0 {
                continue;
            }

            let mut data = self.read_block(block_num)?;
            if remove_entry_from_block(&mut data, name) {
                self.write_block(block_num, &data)?;
                return Ok(());
            }
        }

        Err(Error::new(Errno::ENOENT))
    }

    // ── INode construction ─────────────────────────────────────────

    fn make_inode(self: &Arc<Self>, ino: u32, inode: Ext2Inode) -> INode {
        if inode.is_dir() {
            INode::Directory(Arc::new(Ext2Dir {
                fs: self.clone(),
                inode_num: ino,
                inode: SpinLock::new(inode),
            }))
        } else if inode.is_symlink() {
            INode::Symlink(Arc::new(Ext2Symlink {
                fs: self.clone(),
                inode_num: ino,
                inode: SpinLock::new(inode),
            }))
        } else {
            INode::FileLike(Arc::new(Ext2File {
                fs: self.clone(),
                inode_num: ino,
                inode: SpinLock::new(inode),
            }))
        }
    }

    fn make_stat(&self, ino: u32, inode: &Ext2Inode) -> Stat {
        Stat {
            dev: DevId::new(0),
            inode_no: INodeNo::new(ino as usize),
            nlink: NLink::new(inode.links_count as usize),
            mode: FileMode::new(inode.file_mode_bits()),
            uid: UId::new(inode.uid as u32),
            gid: GId::new(inode.gid as u32),
            pad0: 0,
            rdev: DevId::new(0),
            size: FileSize(inode.file_size() as isize),
            blksize: BlockSize::new(self.block_size as isize),
            blocks: BlockCount::new(inode.blocks as isize),
            atime: Time::new(inode.atime as isize),
            atime_nsec: Time::new(0),
            mtime: Time::new(inode.mtime as isize),
            mtime_nsec: Time::new(0),
            ctime: Time::new(inode.ctime as isize),
            ctime_nsec: Time::new(0),
            _unused: [0; 3],
        }
    }
}

// ── Public filesystem wrapper ──────────────────────────────────────

/// The ext2/ext3/ext4 filesystem.
pub struct Ext2Filesystem {
    inner: Arc<Ext2Inner>,
}

impl Ext2Filesystem {
    /// Mount an ext2/ext3/ext4 filesystem from a block device.
    pub fn mount(device: Arc<dyn BlockDevice>) -> Result<Arc<Ext2Filesystem>> {
        // Read superblock at offset 1024.
        let sb_sector = SUPERBLOCK_OFFSET / 512;
        let sb_sectors = (SUPERBLOCK_SIZE + 511) / 512;
        let mut sb_buf = vec![0u8; sb_sectors * 512];
        device
            .read_sectors(sb_sector, &mut sb_buf)
            .map_err(|_| Error::new(Errno::EIO))?;

        let superblock = Ext2Superblock::parse(&sb_buf[..SUPERBLOCK_SIZE])
            .ok_or_else(|| Error::new(Errno::EINVAL))?;

        let block_size = superblock.block_size();
        let inode_size = superblock.inode_size as usize;
        let inodes_per_group = superblock.inodes_per_group;
        let is_64bit = superblock.is_64bit();
        let group_desc_size = if is_64bit {
            core::cmp::max(superblock.desc_size as usize, 64)
        } else {
            GROUP_DESC_SIZE_32
        };

        log::info!(
            "{}: mounted ({} blocks, {} inodes, block_size={}, inode_size={}, desc_size={})",
            superblock.label(),
            superblock.blocks_count,
            superblock.inodes_count,
            block_size,
            inode_size,
            group_desc_size,
        );

        // Read block group descriptor table.
        let gdt_block = if block_size == 1024 { 2 } else { 1 };
        let num_groups =
            (superblock.blocks_count + superblock.blocks_per_group - 1)
                / superblock.blocks_per_group;
        let gdt_bytes = num_groups as usize * group_desc_size;
        let gdt_buf = read_blocks_raw(&device, gdt_block as u64, gdt_bytes, block_size)?;

        let mut groups = Vec::with_capacity(num_groups as usize);
        for i in 0..num_groups as usize {
            let offset = i * group_desc_size;
            let end = min(offset + group_desc_size, gdt_buf.len());
            groups.push(Ext2GroupDesc::parse(&gdt_buf[offset..end], is_64bit));
        }

        let free_blocks_count = superblock.free_blocks_count;
        let free_inodes_count = superblock.free_inodes_count;
        let blocks_per_group = superblock.blocks_per_group;
        let first_data_block = superblock.first_data_block;

        Ok(Arc::new(Ext2Filesystem {
            inner: Arc::new(Ext2Inner {
                device,
                superblock,
                block_size,
                inodes_per_group,
                inode_size,
                is_64bit,
                group_desc_size,
                blocks_per_group,
                first_data_block,
                state: SpinLock::new(Ext2MutableState {
                    groups,
                    free_blocks_count,
                    free_inodes_count,
                }),
                dev_id: kevlar_vfs::inode::alloc_dev_id(),
            }),
        }))
    }
}

impl FileSystem for Ext2Filesystem {
    fn root_dir(&self) -> Result<Arc<dyn Directory>> {
        let inode = self.inner.read_inode(EXT2_ROOT_INO)?;
        Ok(Arc::new(Ext2Dir {
            fs: self.inner.clone(),
            inode_num: EXT2_ROOT_INO,
            inode: SpinLock::new(inode),
        }))
    }
}

// ── Parsed directory entry ─────────────────────────────────────────

struct Ext2DirEntryInfo {
    inode: u32,
    file_type: u8,
    name: String,
}

// ── Ext2Dir ────────────────────────────────────────────────────────

struct Ext2Dir {
    fs: Arc<Ext2Inner>,
    inode_num: u32,
    inode: SpinLock<Ext2Inode>,
}

impl Directory for Ext2Dir {
    fn lookup(&self, name: &str) -> Result<INode> {
        let inode = self.inode.lock_no_irq();
        let entries = self.fs.read_dir_entries(&inode)?;
        drop(inode);
        for entry in &entries {
            if entry.name == name {
                let child_inode = self.fs.read_inode(entry.inode)?;
                return Ok(self.fs.make_inode(entry.inode, child_inode));
            }
        }
        Err(Error::new(Errno::ENOENT))
    }

    fn create_file(&self, name: &str, mode: FileMode, uid: UId, gid: GId) -> Result<INode> {
        // Check for existing entry.
        {
            let inode = self.inode.lock_no_irq();
            let entries = self.fs.read_dir_entries(&inode)?;
            for entry in &entries {
                if entry.name == name {
                    return Err(Error::new(Errno::EEXIST));
                }
            }
        }

        // Allocate inode.
        let ino = self.fs.alloc_inode(false)?;

        // Initialize the inode as a regular file.
        let use_extents = self.fs.superblock.has_extents();
        let new_inode = Ext2Inode {
            mode: EXT2_S_IFREG | (mode.as_u32() as u16 & 0o7777),
            uid: uid.as_u32() as u16,
            size: 0,
            atime: 0,
            ctime: 0,
            mtime: 0,
            gid: gid.as_u32() as u16,
            links_count: 1,
            blocks: 0,
            flags: if use_extents { EXT4_EXTENTS_FL } else { 0 },
            block: if use_extents { Ext2Inner::init_extent_root() } else { [0; 15] },
            size_high: 0,
        };
        self.fs.write_inode(ino, &new_inode)?;

        // Add directory entry.
        {
            let mut dir_inode = self.inode.lock_no_irq();
            self.fs
                .add_dir_entry(self.inode_num, &mut dir_inode, ino, name, EXT2_FT_REG_FILE)?;
        }

        Ok(self.fs.make_inode(ino, new_inode))
    }

    fn create_dir(&self, name: &str, mode: FileMode, uid: UId, gid: GId) -> Result<INode> {
        // Check for existing entry.
        {
            let inode = self.inode.lock_no_irq();
            let entries = self.fs.read_dir_entries(&inode)?;
            for entry in &entries {
                if entry.name == name {
                    return Err(Error::new(Errno::EEXIST));
                }
            }
        }

        let ino = self.fs.alloc_inode(true)?;

        // Allocate a block for . and .. entries.
        let use_extents = self.fs.superblock.has_extents();
        let mut new_inode = Ext2Inode {
            mode: EXT2_S_IFDIR | (mode.as_u32() as u16 & 0o7777),
            uid: uid.as_u32() as u16,
            size: 0,
            atime: 0,
            ctime: 0,
            mtime: 0,
            gid: gid.as_u32() as u16,
            links_count: 2, // . and parent's entry
            blocks: 0,
            flags: if use_extents { EXT4_EXTENTS_FL } else { 0 },
            block: if use_extents { Ext2Inner::init_extent_root() } else { [0; 15] },
            size_high: 0,
        };

        let data_block = if use_extents {
            self.fs.alloc_extent_block(&mut new_inode, ino, 0)?
        } else {
            self.fs.alloc_data_block(&mut new_inode, 0)?
        };
        new_inode.set_file_size(self.fs.block_size as u64);

        // Write . and .. entries.
        let bs = self.fs.block_size;
        let mut block_data = vec![0u8; bs];

        // "." entry
        let dot_rec_len = 12u16; // minimum for "." name
        write_u32(&mut block_data, 0, ino);
        write_u16(&mut block_data, 4, dot_rec_len);
        block_data[6] = 1; // name_len
        block_data[7] = EXT2_FT_DIR;
        block_data[8] = b'.';

        // ".." entry — takes rest of block
        let dotdot_offset = dot_rec_len as usize;
        let dotdot_rec_len = (bs - dotdot_offset) as u16;
        write_u32(&mut block_data, dotdot_offset, self.inode_num);
        write_u16(&mut block_data, dotdot_offset + 4, dotdot_rec_len);
        block_data[dotdot_offset + 6] = 2; // name_len
        block_data[dotdot_offset + 7] = EXT2_FT_DIR;
        block_data[dotdot_offset + 8] = b'.';
        block_data[dotdot_offset + 9] = b'.';

        self.fs.write_block(data_block, &block_data)?;
        self.fs.write_inode(ino, &new_inode)?;

        // Add entry to parent directory + increment parent link count.
        {
            let mut dir_inode = self.inode.lock_no_irq();
            self.fs
                .add_dir_entry(self.inode_num, &mut dir_inode, ino, name, EXT2_FT_DIR)?;
            dir_inode.links_count += 1; // child's ".." points to us
            self.fs.write_inode(self.inode_num, &dir_inode)?;
        }

        Ok(self.fs.make_inode(ino, new_inode))
    }

    fn stat(&self) -> Result<Stat> {
        let inode = self.inode.lock_no_irq();
        Ok(self.fs.make_stat(self.inode_num, &inode))
    }

    fn inode_no(&self) -> Result<INodeNo> {
        Ok(INodeNo::new(self.inode_num as usize))
    }

    fn dev_id(&self) -> usize {
        self.fs.dev_id
    }

    fn readdir(&self, index: usize) -> Result<Option<DirEntry>> {
        let inode = self.inode.lock_no_irq();
        let entries = self.fs.read_dir_entries(&inode)?;
        let filtered: Vec<_> = entries
            .iter()
            .filter(|e| e.name != "." && e.name != "..")
            .collect();

        let entry = filtered.get(index).map(|e| {
            let ft = match e.file_type {
                EXT2_FT_DIR => FileType::Directory,
                EXT2_FT_SYMLINK => FileType::Link,
                _ => FileType::Regular,
            };
            DirEntry {
                inode_no: INodeNo::new(e.inode as usize),
                file_type: ft,
                name: e.name.clone(),
            }
        });

        Ok(entry)
    }

    fn link(&self, name: &str, link_to: &INode) -> Result<()> {
        // Get inode number from target.
        let target_stat = link_to.stat()?;
        let target_ino = target_stat.inode_no.as_u64() as u32;

        // Read target inode to determine file type and increment links.
        let mut target_inode = self.fs.read_inode(target_ino)?;
        if target_inode.is_dir() {
            return Err(Error::new(Errno::EPERM)); // Can't hard-link directories.
        }

        let ft = if target_inode.is_symlink() {
            EXT2_FT_SYMLINK
        } else {
            EXT2_FT_REG_FILE
        };

        // Add directory entry.
        {
            let mut dir_inode = self.inode.lock_no_irq();
            self.fs
                .add_dir_entry(self.inode_num, &mut dir_inode, target_ino, name, ft)?;
        }

        // Increment link count.
        target_inode.links_count += 1;
        self.fs.write_inode(target_ino, &target_inode)?;

        Ok(())
    }

    fn create_symlink(&self, name: &str, target: &str) -> Result<INode> {
        // Check for existing entry.
        {
            let inode = self.inode.lock_no_irq();
            let entries = self.fs.read_dir_entries(&inode)?;
            for entry in &entries {
                if entry.name == name {
                    return Err(Error::new(Errno::EEXIST));
                }
            }
        }

        let ino = self.fs.alloc_inode(false)?;
        let target_bytes = target.as_bytes();
        let target_len = target_bytes.len();

        // Inline symlinks (<= 60 bytes) store target in i_block — can't use extents.
        // Block-based symlinks (> 60 bytes) use extents when available.
        let use_extents = self.fs.superblock.has_extents() && target_len > 60;
        let mut new_inode = Ext2Inode {
            mode: EXT2_S_IFLNK | 0o777,
            uid: 0,
            size: target_len as u32,
            atime: 0,
            ctime: 0,
            mtime: 0,
            gid: 0,
            links_count: 1,
            blocks: 0,
            flags: if use_extents { EXT4_EXTENTS_FL } else { 0 },
            block: if use_extents { Ext2Inner::init_extent_root() } else { [0; 15] },
            size_high: 0,
        };

        if target_len <= 60 {
            // Inline symlink: store target in i_block[0..14] (60 bytes).
            let block_bytes: &mut [u32; 15] = &mut new_inode.block;
            let mut buf = [0u8; 60];
            buf[..target_len].copy_from_slice(target_bytes);
            for (i, chunk) in buf.chunks(4).enumerate() {
                block_bytes[i] = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            }
        } else {
            // Block-based symlink.
            let data_block = if use_extents {
                self.fs.alloc_extent_block(&mut new_inode, ino, 0)?
            } else {
                self.fs.alloc_data_block(&mut new_inode, 0)?
            };
            let mut block_data = vec![0u8; self.fs.block_size];
            block_data[..target_len].copy_from_slice(target_bytes);
            self.fs.write_block(data_block, &block_data)?;
        }

        self.fs.write_inode(ino, &new_inode)?;

        // Add directory entry.
        {
            let mut dir_inode = self.inode.lock_no_irq();
            self.fs
                .add_dir_entry(self.inode_num, &mut dir_inode, ino, name, EXT2_FT_SYMLINK)?;
        }

        Ok(self.fs.make_inode(ino, new_inode))
    }

    fn unlink(&self, name: &str) -> Result<()> {
        // Lookup the entry.
        let target_ino;
        {
            let inode = self.inode.lock_no_irq();
            let entries = self.fs.read_dir_entries(&inode)?;
            let entry = entries
                .iter()
                .find(|e| e.name == name)
                .ok_or_else(|| Error::new(Errno::ENOENT))?;
            if entry.file_type == EXT2_FT_DIR {
                return Err(Error::new(Errno::EISDIR));
            }
            target_ino = entry.inode;
        }

        // Remove directory entry.
        {
            let dir_inode = self.inode.lock_no_irq();
            self.fs.remove_dir_entry(&dir_inode, name)?;
        }

        // Decrement link count; free inode+blocks if it reaches 0.
        let mut target_inode = self.fs.read_inode(target_ino)?;
        target_inode.links_count = target_inode.links_count.saturating_sub(1);
        if target_inode.links_count == 0 {
            self.fs.free_file_blocks(&target_inode)?;
            target_inode.set_file_size(0);
            target_inode.blocks = 0;
            target_inode.block = [0; 15];
        }
        self.fs.write_inode(target_ino, &target_inode)?;

        if target_inode.links_count == 0 {
            self.fs.free_inode(target_ino, false)?;
        }

        Ok(())
    }

    fn rmdir(&self, name: &str) -> Result<()> {
        if name == "." || name == ".." {
            return Err(Error::new(Errno::EINVAL));
        }

        // Lookup the entry.
        let target_ino;
        {
            let inode = self.inode.lock_no_irq();
            let entries = self.fs.read_dir_entries(&inode)?;
            let entry = entries
                .iter()
                .find(|e| e.name == name)
                .ok_or_else(|| Error::new(Errno::ENOENT))?;
            if entry.file_type != EXT2_FT_DIR {
                return Err(Error::new(Errno::ENOTDIR));
            }
            target_ino = entry.inode;
        }

        // Check directory is empty (only . and ..).
        let target_inode = self.fs.read_inode(target_ino)?;
        let entries = self.fs.read_dir_entries(&target_inode)?;
        for e in &entries {
            if e.name != "." && e.name != ".." {
                return Err(Error::new(Errno::ENOTEMPTY));
            }
        }

        // Remove directory entry from parent.
        {
            let dir_inode = self.inode.lock_no_irq();
            self.fs.remove_dir_entry(&dir_inode, name)?;
        }

        // Free target's blocks and inode.
        self.fs.free_file_blocks(&target_inode)?;
        let mut dead = target_inode;
        dead.links_count = 0;
        dead.set_file_size(0);
        dead.blocks = 0;
        dead.block = [0; 15];
        self.fs.write_inode(target_ino, &dead)?;
        self.fs.free_inode(target_ino, true)?;

        // Decrement parent link count (child's ".." is gone).
        {
            let mut dir_inode = self.inode.lock_no_irq();
            dir_inode.links_count = dir_inode.links_count.saturating_sub(1);
            self.fs.write_inode(self.inode_num, &dir_inode)?;
        }

        Ok(())
    }

    fn rename(&self, old_name: &str, new_dir: &Arc<dyn Directory>, new_name: &str) -> Result<()> {
        // Same-dir rename only for MVP.
        let new_dir_ino = new_dir.inode_no()?;
        if new_dir_ino.as_u64() != self.inode_num as u64 {
            return Err(Error::new(Errno::EXDEV));
        }

        // Find the old entry.
        let (target_ino, file_type);
        {
            let inode = self.inode.lock_no_irq();
            let entries = self.fs.read_dir_entries(&inode)?;
            let entry = entries
                .iter()
                .find(|e| e.name == old_name)
                .ok_or_else(|| Error::new(Errno::ENOENT))?;
            target_ino = entry.inode;
            file_type = entry.file_type;
        }

        // If new_name already exists, unlink it first.
        {
            let inode = self.inode.lock_no_irq();
            let entries = self.fs.read_dir_entries(&inode)?;
            if let Some(existing) = entries.iter().find(|e| e.name == new_name) {
                if existing.file_type == EXT2_FT_DIR {
                    // Check empty before removing.
                    let existing_inode = self.fs.read_inode(existing.inode)?;
                    let sub_entries = self.fs.read_dir_entries(&existing_inode)?;
                    for e in &sub_entries {
                        if e.name != "." && e.name != ".." {
                            return Err(Error::new(Errno::ENOTEMPTY));
                        }
                    }
                }
                drop(inode);
                // Remove old destination.
                if file_type == EXT2_FT_DIR {
                    // Use internal removal (don't go through rmdir which re-checks).
                    let dir_inode = self.inode.lock_no_irq();
                    self.fs.remove_dir_entry(&dir_inode, new_name)?;
                } else {
                    let dir_inode = self.inode.lock_no_irq();
                    self.fs.remove_dir_entry(&dir_inode, new_name)?;
                }
            }
        }

        // Remove old entry.
        {
            let dir_inode = self.inode.lock_no_irq();
            self.fs.remove_dir_entry(&dir_inode, old_name)?;
        }

        // Add new entry.
        {
            let mut dir_inode = self.inode.lock_no_irq();
            self.fs.add_dir_entry(
                self.inode_num,
                &mut dir_inode,
                target_ino,
                new_name,
                file_type,
            )?;
        }

        Ok(())
    }
}

impl fmt::Debug for Ext2Dir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ext2Dir")
            .field("inode", &self.inode_num)
            .finish()
    }
}

// ── Ext2File ───────────────────────────────────────────────────────

struct Ext2File {
    fs: Arc<Ext2Inner>,
    inode_num: u32,
    inode: SpinLock<Ext2Inode>,
}

impl FileLike for Ext2File {
    fn read(
        &self,
        offset: usize,
        buf: UserBufferMut<'_>,
        _options: &OpenOptions,
    ) -> Result<usize> {
        let inode = self.inode.lock_no_irq();
        let mut writer = UserBufWriter::from(buf);
        let data = self.fs.read_file_data(&inode, offset, writer.remaining_len())?;
        writer.write_bytes(&data)
    }

    fn write(
        &self,
        offset: usize,
        buf: UserBuffer<'_>,
        _options: &OpenOptions,
    ) -> Result<usize> {
        let mut reader = UserBufReader::from(buf);
        let write_len = reader.remaining_len();
        if write_len == 0 {
            return Ok(0);
        }

        // Read all data from user buffer first.
        let mut user_data = vec![0u8; write_len];
        reader.read_bytes(&mut user_data)?;

        let mut inode = self.inode.lock_no_irq();
        let ptrs_per_block = self.fs.block_size / 4;
        let bs = self.fs.block_size;
        let mut pos = offset;
        let mut src_off = 0;

        while src_off < write_len {
            let block_index = pos / bs;
            let offset_in_block = pos % bs;
            let chunk_len = min(write_len - src_off, bs - offset_in_block);

            // Resolve or allocate the block.
            let use_extents = inode.uses_extents();
            let mut block_num = if use_extents {
                self.fs.resolve_extent(&inode, block_index)?
            } else {
                self.fs.resolve_block_ptr(&inode, block_index, ptrs_per_block)? as u64
            };

            if block_num == 0 {
                // Need to allocate a new block.
                if use_extents {
                    block_num = self.fs.alloc_extent_block(&mut inode, self.inode_num, block_index)?;
                } else {
                    block_num = self.fs.alloc_data_block(&mut inode, block_index)?;
                }
            }

            // Read-modify-write for partial blocks, direct write for full blocks.
            if chunk_len == bs {
                self.fs.write_block(block_num, &user_data[src_off..src_off + chunk_len])?;
            } else {
                let mut block_data = self.fs.read_block(block_num)?;
                block_data[offset_in_block..offset_in_block + chunk_len]
                    .copy_from_slice(&user_data[src_off..src_off + chunk_len]);
                self.fs.write_block(block_num, &block_data)?;
            }

            pos += chunk_len;
            src_off += chunk_len;
        }

        // Update size if we grew.
        let new_end = (offset + write_len) as u64;
        if new_end > inode.file_size() {
            inode.set_file_size(new_end);
        }
        self.fs.write_inode(self.inode_num, &inode)?;

        Ok(write_len)
    }

    fn stat(&self) -> Result<Stat> {
        let inode = self.inode.lock_no_irq();
        Ok(self.fs.make_stat(self.inode_num, &inode))
    }

    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus::POLLIN | PollStatus::POLLOUT)
    }

    fn truncate(&self, length: usize) -> Result<()> {
        let mut inode = self.inode.lock_no_irq();
        let old_size = inode.file_size() as usize;
        let bs = self.fs.block_size;

        // Fast path for truncate(0) on extent files: free all blocks and
        // reinitialize an empty extent tree. This is the common O_TRUNC case.
        if length == 0 && inode.uses_extents() && old_size > 0 {
            self.fs.free_extent_blocks(&inode)?;
            inode.block = Ext2Inner::init_extent_root();
            inode.blocks = 0;
            inode.set_file_size(0);
            self.fs.write_inode(self.inode_num, &inode)?;
            return Ok(());
        }

        if length < old_size {
            // Free blocks beyond the new size.
            let new_block_count = (length + bs - 1) / bs;
            let old_block_count = (old_size + bs - 1) / bs;
            let ptrs_per_block = bs / 4;

            for blk_idx in new_block_count..old_block_count {
                let block_num = if inode.uses_extents() {
                    self.fs.resolve_extent(&inode, blk_idx)?
                } else {
                    self.fs.resolve_block_ptr(&inode, blk_idx, ptrs_per_block)? as u64
                };
                if block_num != 0 {
                    self.fs.free_block(block_num)?;
                    inode.blocks = inode.blocks.saturating_sub((bs / 512) as u32);
                    // Clear the block pointer.
                    if !inode.uses_extents() {
                        self.fs.set_block_ptr(&mut inode, blk_idx, 0)?;
                    }
                }
            }

            // Zero the partial last block if needed.
            if length > 0 && length % bs != 0 {
                let last_blk_idx = (length - 1) / bs;
                let block_num = if inode.uses_extents() {
                    self.fs.resolve_extent(&inode, last_blk_idx)?
                } else {
                    self.fs.resolve_block_ptr(&inode, last_blk_idx, ptrs_per_block)? as u64
                };
                if block_num != 0 {
                    let mut block_data = self.fs.read_block(block_num)?;
                    let zero_start = length % bs;
                    for b in &mut block_data[zero_start..] {
                        *b = 0;
                    }
                    self.fs.write_block(block_num, &block_data)?;
                }
            }
        }

        inode.set_file_size(length as u64);
        self.fs.write_inode(self.inode_num, &inode)?;
        Ok(())
    }
}

impl fmt::Debug for Ext2File {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ext2File")
            .field("inode", &self.inode_num)
            .finish()
    }
}

// ── Ext2Symlink ────────────────────────────────────────────────────

struct Ext2Symlink {
    fs: Arc<Ext2Inner>,
    inode_num: u32,
    inode: SpinLock<Ext2Inode>,
}

impl SymlinkTrait for Ext2Symlink {
    fn stat(&self) -> Result<Stat> {
        let inode = self.inode.lock_no_irq();
        Ok(self.fs.make_stat(self.inode_num, &inode))
    }

    fn linked_to(&self) -> Result<Cow<'_, str>> {
        let inode = self.inode.lock_no_irq();
        let size = inode.file_size() as usize;
        if size <= 60 && !inode.uses_extents() {
            // Inline symlink: target stored in i_block[0..14] (60 bytes).
            // Use a stack buffer instead of Vec to avoid heap allocation.
            let mut buf = [0u8; 60];
            let mut pos = 0;
            for b in &inode.block {
                let bytes = b.to_le_bytes();
                let remaining = size.saturating_sub(pos);
                let copy_len = core::cmp::min(bytes.len(), remaining);
                if copy_len == 0 { break; }
                buf[pos..pos + copy_len].copy_from_slice(&bytes[..copy_len]);
                pos += copy_len;
            }
            let target =
                core::str::from_utf8(&buf[..size]).map_err(|_| Error::new(Errno::EIO))?;
            Ok(Cow::Owned(String::from(target)))
        } else {
            // Block-based symlink.
            let data = self.fs.read_file_data(&inode, 0, size)?;
            let target =
                core::str::from_utf8(&data).map_err(|_| Error::new(Errno::EIO))?;
            Ok(Cow::Owned(String::from(target)))
        }
    }
}

impl fmt::Debug for Ext2Symlink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ext2Symlink")
            .field("inode", &self.inode_num)
            .finish()
    }
}

// ── Public mount function ──────────────────────────────────────────

/// Mount an ext2/ext3/ext4 filesystem from the global block device.
pub fn mount_ext2() -> Result<Arc<Ext2Filesystem>> {
    let device = block_device().ok_or_else(|| Error::new(Errno::ENODEV))?;
    Ext2Filesystem::mount(device)
}

// ── Helper functions ───────────────────────────────────────────────

fn read_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn write_u16(data: &mut [u8], offset: usize, val: u16) {
    let bytes = val.to_le_bytes();
    data[offset] = bytes[0];
    data[offset + 1] = bytes[1];
}

fn write_u32(data: &mut [u8], offset: usize, val: u32) {
    let bytes = val.to_le_bytes();
    data[offset] = bytes[0];
    data[offset + 1] = bytes[1];
    data[offset + 2] = bytes[2];
    data[offset + 3] = bytes[3];
}

/// Compute the on-disk size of a directory entry with the given name length,
/// aligned to 4 bytes.
fn dir_entry_len(name_len: usize) -> usize {
    (8 + name_len + 3) & !3
}

/// Find the first free bit in a bitmap buffer, up to max_bits.
fn find_free_bit(bitmap: &[u8], max_bits: usize) -> Option<usize> {
    find_free_bit_from(bitmap, 0, max_bits)
}

/// Find the first free bit starting from `start_bit`, wrapping around to 0.
fn find_free_bit_from(bitmap: &[u8], start_bit: usize, max_bits: usize) -> Option<usize> {
    // Search from start_bit to max_bits.
    for bit in start_bit..max_bits {
        if bitmap[bit / 8] & (1 << (bit % 8)) == 0 {
            return Some(bit);
        }
    }
    // Wrap around: search from 0 to start_bit.
    for bit in 0..start_bit {
        if bitmap[bit / 8] & (1 << (bit % 8)) == 0 {
            return Some(bit);
        }
    }
    None
}

/// Try to insert a directory entry into an existing block by splitting rec_len.
/// Returns Some(()) on success, None if no space was found.
fn try_insert_dir_entry(
    data: &mut [u8],
    child_ino: u32,
    name: &str,
    file_type: u8,
    needed: usize,
) -> Option<()> {
    let block_size = data.len();
    let mut pos = 0;

    while pos + 8 <= block_size {
        let ino = read_u32(data, pos);
        let rec_len = read_u16(data, pos + 4) as usize;
        if rec_len == 0 || rec_len < 8 {
            return None;
        }

        let actual = if ino == 0 {
            0 // Deleted entry — entire rec_len is available.
        } else {
            let name_len = data[pos + 6] as usize;
            dir_entry_len(name_len)
        };

        let free_space = rec_len - actual;
        if free_space >= needed {
            if ino != 0 {
                // Shrink existing entry.
                write_u16(data, pos + 4, actual as u16);
                // Write new entry after it.
                let new_pos = pos + actual;
                let new_rec_len = rec_len - actual;
                write_u32(data, new_pos, child_ino);
                write_u16(data, new_pos + 4, new_rec_len as u16);
                data[new_pos + 6] = name.len() as u8;
                data[new_pos + 7] = file_type;
                data[new_pos + 8..new_pos + 8 + name.len()].copy_from_slice(name.as_bytes());
            } else {
                // Reuse deleted entry.
                write_u32(data, pos, child_ino);
                // Keep rec_len as-is (preserves chain).
                data[pos + 6] = name.len() as u8;
                data[pos + 7] = file_type;
                data[pos + 8..pos + 8 + name.len()].copy_from_slice(name.as_bytes());
            }
            return Some(());
        }

        pos += rec_len;
    }

    None
}

/// Remove a directory entry by name from a block. Returns true if found.
fn remove_entry_from_block(data: &mut [u8], name: &str) -> bool {
    let block_size = data.len();
    let mut pos = 0;
    let mut prev_pos: Option<usize> = None;

    while pos + 8 <= block_size {
        let ino = read_u32(data, pos);
        let rec_len = read_u16(data, pos + 4) as usize;
        if rec_len == 0 || rec_len < 8 {
            return false;
        }

        if ino != 0 {
            let name_len = data[pos + 6] as usize;
            if name_len == name.len() && pos + 8 + name_len <= block_size {
                let entry_name = &data[pos + 8..pos + 8 + name_len];
                if entry_name == name.as_bytes() {
                    // Found it. Merge with predecessor or zero inode.
                    if let Some(pp) = prev_pos {
                        // Merge: extend predecessor's rec_len.
                        let prev_rec_len = read_u16(data, pp + 4) as usize;
                        write_u16(data, pp + 4, (prev_rec_len + rec_len) as u16);
                    } else {
                        // First entry in block — zero the inode.
                        write_u32(data, pos, 0);
                    }
                    return true;
                }
            }
        }

        prev_pos = Some(pos);
        pos += rec_len;
    }

    false
}

/// Read raw bytes starting at a given block, enough for `byte_len` bytes.
fn read_blocks_raw(
    device: &Arc<dyn BlockDevice>,
    block_num: u64,
    byte_len: usize,
    block_size: usize,
) -> Result<Vec<u8>> {
    let sector = block_num * (block_size as u64 / 512);
    let num_sectors = (byte_len + 511) / 512;
    let mut buf = vec![0u8; num_sectors * 512];
    device
        .read_sectors(sector, &mut buf)
        .map_err(|_| Error::new(Errno::EIO))?;
    buf.truncate(byte_len);
    Ok(buf)
}
