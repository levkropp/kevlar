// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Read-only ext2 filesystem for Kevlar.
//!
//! Clean-room implementation from the publicly documented ext2 on-disk format:
//! - FreeBSD `sys/fs/ext2fs/` (BSD-2-Clause)
//! - "The Second Extended Filesystem" by Dave Poirier
//! - OSDev wiki ext2 page
//!
//! This is a Ring 2 service crate — no unsafe code.
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::cmp::min;
use core::fmt;

use kevlar_api::driver::block::{block_device, BlockDevice, BlockError};
use kevlar_vfs::{
    file_system::FileSystem,
    inode::{
        DirEntry, Directory, FileLike, FileType, INode, INodeNo, OpenOptions, PollStatus,
        Symlink as SymlinkTrait,
    },
    path::PathBuf,
    result::{Errno, Error, Result},
    stat::{BlockCount, BlockSize, DevId, FileMode, FileSize, GId, NLink, Stat, Time, UId,
           S_IFDIR, S_IFLNK, S_IFREG},
    user_buffer::{UserBufWriter, UserBuffer, UserBufferMut},
};

// ext2 magic number.
const EXT2_SUPER_MAGIC: u16 = 0xEF53;

// Superblock offset and size.
const SUPERBLOCK_OFFSET: u64 = 1024;
const SUPERBLOCK_SIZE: usize = 1024;

// Block group descriptor size.
const GROUP_DESC_SIZE: usize = 32;

// ext2 inode mode type bits.
const EXT2_S_IFREG: u16 = 0x8000;
const EXT2_S_IFDIR: u16 = 0x4000;
const EXT2_S_IFLNK: u16 = 0xA000;

// ext2 directory file types.
const EXT2_FT_REG_FILE: u8 = 1;
const EXT2_FT_DIR: u8 = 2;
const EXT2_FT_SYMLINK: u8 = 7;

// Root inode is always 2 in ext2.
const EXT2_ROOT_INO: u32 = 2;

// Number of direct block pointers.
const EXT2_NDIR_BLOCKS: usize = 12;
// Indirect block pointer indices.
const EXT2_IND_BLOCK: usize = 12;
const EXT2_DIND_BLOCK: usize = 13;

/// On-disk ext2 superblock fields we care about.
#[derive(Debug, Clone)]
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
}

impl Ext2Superblock {
    fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 128 {
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
        };
        if sb.magic != EXT2_SUPER_MAGIC {
            return None;
        }
        Some(sb)
    }

    fn block_size(&self) -> usize {
        1024 << self.log_block_size
    }
}

/// On-disk ext2 block group descriptor.
#[derive(Debug, Clone)]
struct Ext2GroupDesc {
    inode_table: u32,
}

impl Ext2GroupDesc {
    fn parse(data: &[u8]) -> Self {
        Ext2GroupDesc {
            inode_table: read_u32(data, 8),
        }
    }
}

/// On-disk ext2 inode.
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
    block: [u32; 15],
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
            block,
        }
    }

    fn is_dir(&self) -> bool {
        (self.mode & 0xF000) == EXT2_S_IFDIR
    }

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
}

/// The ext2 filesystem.
pub struct Ext2Filesystem {
    device: Arc<dyn BlockDevice>,
    superblock: Ext2Superblock,
    block_size: usize,
    groups: Vec<Ext2GroupDesc>,
    inodes_per_group: u32,
    inode_size: usize,
}

impl Ext2Filesystem {
    /// Mount an ext2 filesystem from a block device.
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

        log::info!(
            "ext2: mounted ({} blocks, {} inodes, block_size={}, inode_size={})",
            superblock.blocks_count,
            superblock.inodes_count,
            block_size,
            inode_size,
        );

        // Read block group descriptor table.
        // It starts at the block after the superblock.
        let gdt_block = if block_size == 1024 { 2 } else { 1 };
        let num_groups =
            (superblock.blocks_count + superblock.blocks_per_group - 1) / superblock.blocks_per_group;
        let gdt_bytes = num_groups as usize * GROUP_DESC_SIZE;
        let gdt_buf = read_blocks_raw(&device, gdt_block as u64, gdt_bytes, block_size)?;

        let mut groups = Vec::with_capacity(num_groups as usize);
        for i in 0..num_groups as usize {
            let offset = i * GROUP_DESC_SIZE;
            groups.push(Ext2GroupDesc::parse(&gdt_buf[offset..offset + GROUP_DESC_SIZE]));
        }

        Ok(Arc::new(Ext2Filesystem {
            device,
            superblock,
            block_size,
            groups,
            inodes_per_group,
            inode_size,
        }))
    }

    /// Read an inode by number.
    fn read_inode(&self, ino: u32) -> Result<Ext2Inode> {
        let group = ((ino - 1) / self.inodes_per_group) as usize;
        let index = ((ino - 1) % self.inodes_per_group) as usize;

        if group >= self.groups.len() {
            return Err(Error::new(Errno::EIO));
        }

        let inode_table_block = self.groups[group].inode_table as u64;
        let byte_offset = index * self.inode_size;
        let block_offset = byte_offset / self.block_size;
        let offset_in_block = byte_offset % self.block_size;

        let block_data = self.read_block(inode_table_block + block_offset as u64)?;
        Ok(Ext2Inode::parse(&block_data[offset_in_block..]))
    }

    /// Read a block by block number.
    fn read_block(&self, block_num: u64) -> Result<Vec<u8>> {
        let sector = block_num * (self.block_size as u64 / 512);
        let mut buf = vec![0u8; self.block_size];
        self.device
            .read_sectors(sector, &mut buf)
            .map_err(|_| Error::new(Errno::EIO))?;
        Ok(buf)
    }

    /// Read file data at a given offset for a given length.
    fn read_file_data(&self, inode: &Ext2Inode, offset: usize, len: usize) -> Result<Vec<u8>> {
        let file_size = inode.size as usize;
        if offset >= file_size {
            return Ok(Vec::new());
        }

        let read_len = min(len, file_size - offset);
        let mut result = Vec::with_capacity(read_len);
        let mut remaining = read_len;
        let mut pos = offset;

        let ptrs_per_block = self.block_size / 4;

        while remaining > 0 {
            let block_index = pos / self.block_size;
            let offset_in_block = pos % self.block_size;
            let chunk_len = min(remaining, self.block_size - offset_in_block);

            let block_num = self.resolve_block_ptr(inode, block_index, ptrs_per_block)?;
            if block_num == 0 {
                // Sparse file hole — return zeros.
                result.extend(core::iter::repeat(0u8).take(chunk_len));
            } else {
                let block_data = self.read_block(block_num as u64)?;
                result.extend_from_slice(&block_data[offset_in_block..offset_in_block + chunk_len]);
            }

            pos += chunk_len;
            remaining -= chunk_len;
        }

        Ok(result)
    }

    /// Resolve a logical block index to a physical block number.
    fn resolve_block_ptr(
        &self,
        inode: &Ext2Inode,
        block_index: usize,
        ptrs_per_block: usize,
    ) -> Result<u32> {
        if block_index < EXT2_NDIR_BLOCKS {
            // Direct block.
            return Ok(inode.block[block_index]);
        }

        let index = block_index - EXT2_NDIR_BLOCKS;
        if index < ptrs_per_block {
            // Single indirect.
            let ind_block = inode.block[EXT2_IND_BLOCK];
            if ind_block == 0 {
                return Ok(0);
            }
            let data = self.read_block(ind_block as u64)?;
            return Ok(read_u32(&data, index * 4));
        }

        let index = index - ptrs_per_block;
        if index < ptrs_per_block * ptrs_per_block {
            // Double indirect.
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

        // Triple indirect — not supported.
        Err(Error::new(Errno::EFBIG))
    }

    /// Read all directory entries from a directory inode.
    fn read_dir_entries(&self, inode: &Ext2Inode) -> Result<Vec<Ext2DirEntryInfo>> {
        let data = self.read_file_data(inode, 0, inode.size as usize)?;
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

    /// Create an INode (VFS object) from an ext2 inode.
    fn make_inode(self: &Arc<Self>, ino: u32, inode: &Ext2Inode) -> INode {
        if inode.is_dir() {
            INode::Directory(Arc::new(Ext2Dir {
                fs: self.clone(),
                inode_num: ino,
                inode: inode.clone(),
            }))
        } else if inode.is_symlink() {
            INode::Symlink(Arc::new(Ext2Symlink {
                fs: self.clone(),
                inode_num: ino,
                inode: inode.clone(),
            }))
        } else {
            INode::FileLike(Arc::new(Ext2File {
                fs: self.clone(),
                inode_num: ino,
                inode: inode.clone(),
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
            size: FileSize(inode.size as isize),
            blksize: BlockSize::new(self.block_size as isize),
            blocks: BlockCount::new(inode.blocks as isize),
            atime: Time::new(inode.atime as isize),
            mtime: Time::new(inode.mtime as isize),
            ctime: Time::new(inode.ctime as isize),
        }
    }
}

impl FileSystem for Ext2Filesystem {
    fn root_dir(&self) -> Result<Arc<dyn Directory>> {
        let inode = self.read_inode(EXT2_ROOT_INO)?;
        Ok(Arc::new(Ext2Dir {
            // SAFETY: We need an Arc<Self> but only have &self. The caller
            // always holds an Arc<Ext2Filesystem>, so we reconstruct it here.
            // This is safe because FileSystem is only ever used through Arc.
            fs: Arc::new(Ext2Filesystem {
                device: self.device.clone(),
                superblock: self.superblock.clone(),
                block_size: self.block_size,
                groups: self.groups.clone(),
                inodes_per_group: self.inodes_per_group,
                inode_size: self.inode_size,
            }),
            inode_num: EXT2_ROOT_INO,
            inode,
        }))
    }
}

/// Parsed directory entry info.
struct Ext2DirEntryInfo {
    inode: u32,
    file_type: u8,
    name: String,
}

/// ext2 directory (implements VFS Directory trait).
struct Ext2Dir {
    fs: Arc<Ext2Filesystem>,
    inode_num: u32,
    inode: Ext2Inode,
}

impl Directory for Ext2Dir {
    fn lookup(&self, name: &str) -> Result<INode> {
        let entries = self.fs.read_dir_entries(&self.inode)?;
        for entry in &entries {
            if entry.name == name {
                let child_inode = self.fs.read_inode(entry.inode)?;
                return Ok(self.fs.make_inode(entry.inode, &child_inode));
            }
        }
        Err(Error::new(Errno::ENOENT))
    }

    fn create_file(&self, _name: &str, _mode: FileMode) -> Result<INode> {
        Err(Error::new(Errno::EROFS))
    }

    fn create_dir(&self, _name: &str, _mode: FileMode) -> Result<INode> {
        Err(Error::new(Errno::EROFS))
    }

    fn stat(&self) -> Result<Stat> {
        Ok(self.fs.make_stat(self.inode_num, &self.inode))
    }

    fn inode_no(&self) -> Result<INodeNo> {
        Ok(INodeNo::new(self.inode_num as usize))
    }

    fn readdir(&self, index: usize) -> Result<Option<DirEntry>> {
        let entries = self.fs.read_dir_entries(&self.inode)?;
        // Skip "." and ".." entries.
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

    fn link(&self, _name: &str, _link_to: &INode) -> Result<()> {
        Err(Error::new(Errno::EROFS))
    }

    fn create_symlink(&self, _name: &str, _target: &str) -> Result<INode> {
        Err(Error::new(Errno::EROFS))
    }

    fn unlink(&self, _name: &str) -> Result<()> {
        Err(Error::new(Errno::EROFS))
    }

    fn rmdir(&self, _name: &str) -> Result<()> {
        Err(Error::new(Errno::EROFS))
    }

    fn rename(&self, _old: &str, _new_dir: &Arc<dyn Directory>, _new: &str) -> Result<()> {
        Err(Error::new(Errno::EROFS))
    }
}

impl fmt::Debug for Ext2Dir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ext2Dir")
            .field("inode", &self.inode_num)
            .finish()
    }
}

/// ext2 regular file (implements VFS FileLike trait).
struct Ext2File {
    fs: Arc<Ext2Filesystem>,
    inode_num: u32,
    inode: Ext2Inode,
}

impl FileLike for Ext2File {
    fn read(
        &self,
        offset: usize,
        buf: UserBufferMut<'_>,
        _options: &OpenOptions,
    ) -> Result<usize> {
        let mut writer = UserBufWriter::from(buf);
        let data = self.fs.read_file_data(&self.inode, offset, writer.remaining_len())?;
        writer.write_bytes(&data)
    }

    fn write(
        &self,
        _offset: usize,
        _buf: UserBuffer<'_>,
        _options: &OpenOptions,
    ) -> Result<usize> {
        Err(Error::new(Errno::EROFS))
    }

    fn stat(&self) -> Result<Stat> {
        Ok(self.fs.make_stat(self.inode_num, &self.inode))
    }

    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus::POLLIN | PollStatus::POLLOUT)
    }
}

impl fmt::Debug for Ext2File {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ext2File")
            .field("inode", &self.inode_num)
            .field("size", &self.inode.size)
            .finish()
    }
}

/// ext2 symbolic link (implements VFS Symlink trait).
struct Ext2Symlink {
    fs: Arc<Ext2Filesystem>,
    inode_num: u32,
    inode: Ext2Inode,
}

impl SymlinkTrait for Ext2Symlink {
    fn stat(&self) -> Result<Stat> {
        Ok(self.fs.make_stat(self.inode_num, &self.inode))
    }

    fn linked_to(&self) -> Result<PathBuf> {
        let size = self.inode.size as usize;
        if size <= 60 {
            // Inline symlink: target stored in i_block[0..14] (60 bytes).
            let block_bytes: Vec<u8> = self.inode.block.iter()
                .flat_map(|b| b.to_le_bytes())
                .collect();
            let target = core::str::from_utf8(&block_bytes[..size])
                .map_err(|_| Error::new(Errno::EIO))?;
            Ok(PathBuf::from(target))
        } else {
            // Block-based symlink: read from data blocks.
            let data = self.fs.read_file_data(&self.inode, 0, size)?;
            let target = core::str::from_utf8(&data)
                .map_err(|_| Error::new(Errno::EIO))?;
            Ok(PathBuf::from(target))
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

/// Mount an ext2 filesystem from the global block device.
pub fn mount_ext2() -> Result<Arc<Ext2Filesystem>> {
    let device = block_device().ok_or_else(|| Error::new(Errno::ENODEV))?;
    Ext2Filesystem::mount(device)
}

// --- Helper functions for reading little-endian fields ---

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
