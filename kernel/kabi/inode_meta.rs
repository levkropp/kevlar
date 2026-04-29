// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Per-inode metadata side-table (Phase 5 v3).
//!
//! Erofs's directory iterator and file-read paths route through
//! `inode->i_mapping->a_ops->read_folio` for layout-aware
//! logical→physical offset translation.  In real Linux, that's
//! `erofs_read_folio` → iomap → `erofs_map_blocks`, which knows
//! about FLAT_PLAIN, FLAT_INLINE, CHUNK_BASED, COMPRESSED layouts.
//!
//! This module implements the kABI equivalent: a `KabiInodeMeta`
//! struct cached per-inode that holds the layout decision and the
//! physical offsets needed to translate.  Populated by
//! `register_inode_from_nid` (called from our `iget5_locked`),
//! consumed by `kabi::filemap::read_cache_folio`'s
//! `resolve_read_request`.
//!
//! ## Inode layout values (from erofs_fs.h)
//!
//!   * `0` FLAT_PLAIN  — uncompressed, contiguous from raw_blkaddr
//!   * `1` COMPRESSED_FULL
//!   * `2` FLAT_INLINE — uncompressed with inline tail-packed data
//!   * `3` COMPRESSED_COMPACT
//!   * `4` CHUNK_BASED
//!
//! v3 implements FLAT_PLAIN + FLAT_INLINE.  Others are deferred.

use alloc::collections::BTreeMap;
use alloc::string::String;

use kevlar_platform::spinlock::SpinLock;

pub const EROFS_INODE_FLAT_PLAIN: u8 = 0;
pub const EROFS_INODE_COMPRESSED_FULL: u8 = 1;
pub const EROFS_INODE_FLAT_INLINE: u8 = 2;
pub const EROFS_INODE_COMPRESSED_COMPACT: u8 = 3;
pub const EROFS_INODE_CHUNK_BASED: u8 = 4;

/// Erofs inode slot size = `1 << EROFS_ISLOTBITS` = 32 bytes for
/// compact inodes (verified — Linux 7.0 default).
pub const EROFS_ISLOTBITS: u8 = 5;
pub const EROFS_INODE_COMPACT_SIZE: u64 = 32;

/// Per-inode metadata cached after we read the on-disk inode.
#[derive(Clone, Debug)]
pub struct KabiInodeMeta {
    /// Physical byte offset of this inode in the backing file
    /// (`= nid << EROFS_ISLOTBITS`).
    pub iloc: u64,
    /// `i_size` from the on-disk inode.
    pub i_size: u64,
    /// EROFS_INODE_* layout value.
    pub layout: u8,
    /// `i_u` field — for FLAT_PLAIN/INLINE this is `raw_blkaddr`.
    pub raw_blkaddr: u32,
    /// Inline xattr body size in bytes (0 if no xattrs).
    pub xattr_isize: u16,
    /// Backing-file path (e.g. `/lib/test.erofs`).
    pub backing_path: String,
}

/// Side-table indexed by inode VA.  Populated when our
/// `iget5_locked` allocates an inode and reads its on-disk
/// metadata.  Walked by `read_cache_folio` to translate
/// (mapping, index) → (path, physical offset).
pub static INODE_META: SpinLock<BTreeMap<usize, KabiInodeMeta>> =
    SpinLock::new(BTreeMap::new());

/// Register an inode by reading its on-disk metadata from
/// `backing_path` at `iloc = nid << 5`, parsing `erofs_inode_compact`,
/// and stashing the result in `INODE_META[inode_ptr]`.
///
/// Returns Ok with the populated meta, Err on read failure or
/// malformed inode.  Caller decides whether to fall back to a
/// raw-bytes path on Err.
pub fn register_inode_from_nid(
    inode_ptr: usize, nid: u64, backing_path: &str,
) -> Result<KabiInodeMeta, ()> {
    let iloc = nid << EROFS_ISLOTBITS;
    let mut buf = [0u8; EROFS_INODE_COMPACT_SIZE as usize];
    let n = match super::filemap::read_initramfs_at(
        backing_path,
        iloc as usize,
        buf.as_mut_ptr(),
        buf.len(),
    ) {
        Ok(n) => n,
        Err(e) => {
            log::warn!(
                "kabi: register_inode_from_nid: read of {} @ {:#x} failed: {:?}",
                backing_path, iloc, e,
            );
            return Err(());
        }
    };
    if n < EROFS_INODE_COMPACT_SIZE as usize {
        log::warn!("kabi: register_inode_from_nid: short read {} < 32", n);
        return Err(());
    }

    // Parse erofs_inode_compact.  All fields little-endian.
    let i_format = u16::from_le_bytes([buf[0], buf[1]]);
    let i_xattr_icount = u16::from_le_bytes([buf[2], buf[3]]);
    let i_size = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]) as u64;
    let i_u = u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]);

    // Layout = bits [3:1] of i_format.
    let layout = ((i_format >> 1) & 0x07) as u8;

    // xattr inline body size in bytes:
    //   0 if icount == 0
    //   else 12 + (icount - 1) * 4
    let xattr_isize: u16 = if i_xattr_icount == 0 {
        0
    } else {
        12 + (i_xattr_icount - 1) * 4
    };

    let meta = KabiInodeMeta {
        iloc,
        i_size,
        layout,
        raw_blkaddr: i_u,
        xattr_isize,
        backing_path: String::from(backing_path),
    };
    log::info!(
        "kabi: inode_meta[{:#x}]: nid={} iloc={:#x} layout={} \
         i_size={} raw_blkaddr={} xattr_isize={}",
        inode_ptr, nid, iloc, layout, i_size, i_u, xattr_isize,
    );
    INODE_META.lock().insert(inode_ptr, meta.clone());
    Ok(meta)
}

/// Look up the meta for an inode pointer.
pub fn lookup_meta(inode_ptr: usize) -> Option<KabiInodeMeta> {
    INODE_META.lock().get(&inode_ptr).cloned()
}

/// Compute the (backing_path, physical_byte_offset, inline_shift)
/// for a given inode + logical page index.  `inline_shift` is the
/// byte offset WITHIN the read buffer at which the inode's
/// logical data begins (non-zero only for FLAT_INLINE on page 0).
///
/// Returns None if the layout isn't supported in v3.
pub fn translate_offset(
    meta: &KabiInodeMeta, index: u64,
) -> Option<(String, u64, u32)> {
    match meta.layout {
        EROFS_INODE_FLAT_PLAIN => {
            // physical = raw_blkaddr * 4096 + index * 4096
            let physical = meta.raw_blkaddr as u64 * 4096 + index * 4096;
            Some((meta.backing_path.clone(), physical, 0))
        }
        EROFS_INODE_FLAT_INLINE => {
            // For inline, ALL data is inside the inode's block.
            // i_size <= 4096 - (iloc % 4096) - 32 - xattr_isize
            // We read the inode's block; the inline data starts at
            //   block_offset + 32 + xattr_isize (within the block,
            //   relative to the inode's offset within that block).
            // Page 0 is the only valid index.
            if index != 0 {
                return None;
            }
            let block_base = (meta.iloc / 4096) * 4096;
            let inode_offset_in_block = (meta.iloc % 4096) as u32;
            let inline_shift =
                inode_offset_in_block + 32 + meta.xattr_isize as u32;
            Some((meta.backing_path.clone(), block_base, inline_shift))
        }
        _ => {
            // CHUNK_BASED (4) and COMPRESSED (1, 3) are deferred.
            log::warn!(
                "kabi: inode_meta: layout {} not supported in v3",
                meta.layout,
            );
            None
        }
    }
}
