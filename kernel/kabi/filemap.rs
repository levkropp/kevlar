// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux page-cache (`filemap`/`folio`) stubs (K33 Phase 2).
//!
//! Linux's filesystem code talks to the page cache through a small
//! handful of canonical entry points.  K33 v1 implementation: a
//! stub layer that returns null/no-op so the loader resolves the
//! symbols; first fs that actually executes a read path gets the
//! real impl (a per-inode hash table backed by Kevlar's existing
//! page allocator).
//!
//! Real Linux uses a per-`address_space` xarray of folios with
//! quite intricate reverse-map work for VM operations.  For
//! read-only filesystem mounts (K33's scope) a simple per-inode
//! hashmap with a SpinLock is correct — pages are private to one
//! inode, no cross-inode aliasing.  Re-evaluate when we add
//! write-back support (K34+).
//!
//! Surface picked from erofs.ko's undefined-symbol list:
//!
//!   * `__filemap_get_folio_mpol`, `pagecache_get_page`,
//!     `find_get_page`, `filemap_alloc_folio_noprof`,
//!     `filemap_add_folio` — inode → folio lookup/insert.
//!   * `filemap_read`, `filemap_splice_read`, `read_cache_folio`
//!     — read-side helpers that dispatch into the fs's
//!     `address_space_operations->read_folio`.
//!   * `folio_unlock`, `__folio_lock`, `__folio_put`,
//!     `folio_end_read`, `mark_page_accessed`, `flush_dcache_*`,
//!     `truncate_inode_pages_final`, `invalidate_mapping_pages`
//!     — folio lifecycle.
//!   * `readahead_expand`, `page_cache_sync_ra` — readahead
//!     trigger; v1 no-op.

use alloc::string::String;
use core::ffi::{c_int, c_void};

use crate::ksym;

// ── folio lookup / allocation ────────────────────────────────────
//
// Phase 2 ERR_PTR safety pass: the v1 stubs returned null pointers,
// which Linux callers don't IS_ERR-check (null is "not in cache" or
// equivalent, then the caller calls read_folio etc.).  But our
// downstream stubs also return null/garbage, and erofs's compiled
// code eventually calls inline `kmap_local_page(folio)` on a
// null-or-garbage folio, producing a Linux-PAGE_OFFSET-relative VA
// we don't have mapped.
//
// Fix: return ERR_PTR(-EIO) from the lookup/alloc paths so erofs's
// `IS_ERR(folio)` checks catch the failure cleanly and the caller
// returns -EIO up through the mount chain instead of dereferencing
// garbage.  Phase 3 replaces these with real folio infrastructure.

#[inline]
fn err_ptr_eio() -> *mut c_void {
    super::block::err_ptr(-5)
}

#[unsafe(no_mangle)]
pub extern "C" fn __filemap_get_folio_mpol(_mapping: *mut c_void,
                                           _index: u64, _fgp_flags: u32,
                                           _gfp: u32,
                                           _mpol: *const c_void) -> *mut c_void {
    log::warn!("kabi: __filemap_get_folio_mpol (stub) — ERR_PTR(-EIO)");
    err_ptr_eio()
}

#[unsafe(no_mangle)]
pub extern "C" fn pagecache_get_page(_mapping: *mut c_void, _index: u64,
                                     _fgp_flags: u32,
                                     _gfp: u32) -> *mut c_void {
    log::warn!("kabi: pagecache_get_page (stub) — ERR_PTR(-EIO)");
    err_ptr_eio()
}

#[unsafe(no_mangle)]
pub extern "C" fn find_get_page(_mapping: *mut c_void,
                                _index: u64) -> *mut c_void {
    // find_get_page returns NULL on miss (NOT an error pointer).
    // Caller responsibility to handle null.
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn filemap_alloc_folio_noprof(_gfp: u32,
                                             _order: u32) -> *mut c_void {
    log::warn!("kabi: filemap_alloc_folio_noprof (stub) — ERR_PTR(-EIO)");
    err_ptr_eio()
}

#[unsafe(no_mangle)]
pub extern "C" fn filemap_add_folio(_mapping: *mut c_void,
                                    _folio: *mut c_void, _index: u64,
                                    _gfp: u32) -> c_int {
    log::warn!("kabi: filemap_add_folio (stub)");
    -22 // -EINVAL
}

ksym!(__filemap_get_folio_mpol);
ksym!(pagecache_get_page);
ksym!(find_get_page);
ksym!(filemap_alloc_folio_noprof);
ksym!(filemap_add_folio);

// ── folio lifecycle ──────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn __folio_lock(_folio: *mut c_void) {
    log::warn!("kabi: __folio_lock (stub)");
}

#[unsafe(no_mangle)]
pub extern "C" fn folio_unlock(_folio: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn __folio_put(_folio: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn folio_end_read(_folio: *mut c_void, _success: bool) {}

#[unsafe(no_mangle)]
pub extern "C" fn flush_dcache_folio(_folio: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn flush_dcache_page(_page: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn copy_highpage(_to: *mut c_void, _from: *mut c_void) {
    log::warn!("kabi: copy_highpage (stub)");
}

ksym!(__folio_lock);
ksym!(folio_unlock);
ksym!(__folio_put);
ksym!(folio_end_read);
ksym!(flush_dcache_folio);
ksym!(flush_dcache_page);
ksym!(copy_highpage);

// ── read helpers ─────────────────────────────────────────────────

/// Minimal `filemap_read` — the page-cache backbone of buffered reads.
/// `ext4_file_read_iter` → `generic_file_read_iter` → `filemap_read`.
///
/// Phase 13 scope: KVEC iter (kernel-buffer destination), single
/// segment, synchronous, no readahead.  Loops one folio at a time:
/// fetches via `read_cache_folio` (which dispatches to the fs's
/// own `read_folio` for ext4), copies bytes from the folio to the
/// kvec destination, advances `iocb->ki_pos` and the iterator.
#[unsafe(no_mangle)]
pub extern "C" fn filemap_read(iocb: *mut c_void, iter: *mut c_void,
                               already_read: isize) -> isize {
    use super::struct_layouts as fl;
    if iocb.is_null() || iter.is_null() {
        return -22; // -EINVAL
    }
    let file = unsafe {
        *(iocb.cast::<u8>().add(fl::KIOCB_KI_FILP_OFF) as *const *mut c_void)
    };
    if file.is_null() { return -22; }
    let mapping = unsafe {
        *(file.cast::<u8>().add(fl::FILE_F_MAPPING_OFF) as *const *mut c_void)
    };
    if mapping.is_null() { return -22; }
    let inode = unsafe {
        *(mapping.cast::<u8>().add(fl::AS_HOST_OFF) as *const *mut c_void)
    };
    if inode.is_null() { return -22; }
    let i_size: i64 = unsafe {
        *(inode.cast::<u8>().add(fl::INODE_I_SIZE_OFF) as *const i64)
    };
    let i_size = i_size.max(0) as u64;

    let iter_type: u8 = unsafe {
        *(iter.cast::<u8>().add(fl::IOV_ITER_TYPE_OFF))
    };
    if iter_type != fl::ITER_KVEC {
        log::warn!(
            "kabi: filemap_read: unsupported iter_type={} (only KVEC)",
            iter_type,
        );
        return -22;
    }

    let mut ki_pos = unsafe {
        *(iocb.cast::<u8>().add(fl::KIOCB_KI_POS_OFF) as *const i64) as u64
    };
    let mut count = unsafe {
        *(iter.cast::<u8>().add(fl::IOV_ITER_COUNT_OFF) as *const usize)
    };
    let mut iov_offset = unsafe {
        *(iter.cast::<u8>().add(fl::IOV_ITER_IOV_OFFSET_OFF) as *const usize)
    };
    let kvec: *mut u8 = unsafe {
        *(iter.cast::<u8>().add(fl::IOV_ITER_KVEC_OFF) as *const *mut u8)
    };
    if kvec.is_null() {
        return -22;
    }
    let kvec_base: *mut u8 = unsafe {
        *(kvec.add(fl::KVEC_IOV_BASE_OFF) as *const *mut u8)
    };
    let kvec_len = unsafe {
        *(kvec.add(fl::KVEC_IOV_LEN_OFF) as *const usize)
    };
    if kvec_base.is_null() || kvec_len == 0 {
        return -22;
    }

    log::info!(
        "kabi: filemap_read: ki_pos={} count={} i_size={} kvec={:p}+{} \
         iov_off={}",
        ki_pos, count, i_size, kvec_base, kvec_len, iov_offset,
    );

    let mut total = already_read.max(0) as usize;
    while count > 0 && ki_pos < i_size {
        let page_idx = ki_pos / 4096;
        let page_off = (ki_pos % 4096) as usize;
        let max_in_page = 4096 - page_off;
        let max_in_file = (i_size - ki_pos) as usize;
        let max_in_kvec = kvec_len.saturating_sub(iov_offset);
        let want = count.min(max_in_page).min(max_in_file).min(max_in_kvec);
        if want == 0 { break; }

        let folio = read_cache_folio(
            mapping, page_idx, core::ptr::null(), file,
        );
        let folio_addr = folio as usize as i64;
        if folio.is_null() || (folio_addr >= -4095 && folio_addr < 0) {
            log::warn!(
                "kabi: filemap_read: read_cache_folio({:p}, {}) failed = {:p}",
                mapping, page_idx, folio,
            );
            return if total > 0 { total as isize } else { -5 };
        }
        let data_va = super::folio_shadow::folio_to_data_va(folio as u64);
        unsafe {
            core::ptr::copy_nonoverlapping(
                data_va.add(page_off),
                kvec_base.add(iov_offset),
                want,
            );
        }
        ki_pos += want as u64;
        iov_offset += want;
        count -= want;
        total += want;
    }

    // Write back the updated state.
    unsafe {
        *(iocb.cast::<u8>().add(fl::KIOCB_KI_POS_OFF) as *mut i64) = ki_pos as i64;
        *(iter.cast::<u8>().add(fl::IOV_ITER_COUNT_OFF) as *mut usize) = count;
        *(iter.cast::<u8>().add(fl::IOV_ITER_IOV_OFFSET_OFF) as *mut usize) = iov_offset;
    }

    log::info!("kabi: filemap_read: returning {} bytes", total);
    total as isize
}

#[unsafe(no_mangle)]
pub extern "C" fn filemap_splice_read(_in_file: *mut c_void, _ppos: *mut u64,
                                      _pipe: *mut c_void, _len: usize,
                                      _flags: u32) -> isize {
    log::warn!("kabi: filemap_splice_read (stub)");
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn read_cache_folio(mapping: *mut c_void, index: u64,
                                   _filler: *const c_void,
                                   file: *mut c_void) -> *mut c_void {
    // Phase 5 v3 + Phase 13: three-tier dispatch.
    //
    // 1. If `mapping->host` is registered in INODE_META, do
    //    erofs-layout translation (FLAT_PLAIN / FLAT_INLINE).
    // 2. Else if `mapping->a_ops->read_folio` is set (and not our
    //    own synth shim — avoids recursion), allocate a folio and
    //    dispatch into the fs's own read_folio (= ext4_read_folio
    //    for ext4 mappings, which decodes extents + submits bh).
    // 3. Else fall back to the raw initramfs read at `index * 4096`
    //    (mount-time superblock-read path; backing file's mapping
    //    points at the raw fs image).
    //
    // For FLAT_INLINE, the inode's logical data lives WITHIN its
    // own block at `inline_offset = (iloc % 4096) + 32 + xattr_isize`.
    // We read the whole 4 KiB block then memcpy bytes
    // `[inline_offset..inline_offset + i_size]` to dst[0..i_size]
    // and zero the tail.

    // Tier 1: erofs inode_meta path.
    let meta_hit = if !mapping.is_null() {
        let host: usize = unsafe {
            *(mapping.cast::<u8>()
                .add(super::struct_layouts::AS_HOST_OFF)
                as *const usize)
        };
        if host != 0 {
            super::inode_meta::lookup_meta(host)
                .and_then(|m| super::inode_meta::translate_offset(&m, index)
                    .map(|(p, ph, sh)| (p, ph, sh, m.i_size)))
        } else {
            None
        }
    } else {
        None
    };

    if meta_hit.is_none() {
        // Tier 2: dispatch to mapping->a_ops->read_folio (ext4 path).
        if let Some(folio) = dispatch_a_ops_read_folio(mapping, file, index) {
            return folio;
        }
    }

    // Tier 3: legacy initramfs raw-read fallback.
    let (path, physical_offset, inline_shift, inline_size) = match meta_hit {
        Some(t) => t,
        None => resolve_read_request(mapping, file, index),
    };

    let (fake_page_va, data_va) = match super::folio_shadow::alloc_folio(
        super::folio_shadow::PG_UPTODATE,
        mapping,
        index,
    ) {
        Some(t) => t,
        None => {
            log::warn!("kabi: read_cache_folio: folio_shadow exhausted");
            return err_ptr_eio();
        }
    };

    if inline_shift == 0 {
        // Direct read at physical_offset.
        if let Err(e) = read_initramfs_at(
            &path, physical_offset as usize, data_va, 4096,
        ) {
            log::warn!(
                "kabi: read_cache_folio: read of {} @ {} failed: {:?}",
                path, physical_offset, e,
            );
            return err_ptr_eio();
        }
        log::info!(
            "kabi: read_cache_folio: fake_page={:#x} data_va={:p} \
             mapping={:p} index={} path={} phys={:#x}",
            fake_page_va, data_va, mapping, index, path, physical_offset,
        );
    } else {
        // FLAT_INLINE: read the whole block, then shift the
        // inline portion to dst[0..inline_size], zero the tail.
        let mut tmp = alloc::vec![0u8; 4096];
        if let Err(e) = read_initramfs_at(
            &path, physical_offset as usize, tmp.as_mut_ptr(), 4096,
        ) {
            log::warn!(
                "kabi: read_cache_folio: inline read of {} @ {} failed: {:?}",
                path, physical_offset, e,
            );
            return err_ptr_eio();
        }
        let copy_len = (inline_size as usize)
            .min(4096_usize.saturating_sub(inline_shift as usize));
        unsafe {
            // Zero the destination first (alloc_folio already did).
            core::ptr::copy_nonoverlapping(
                tmp.as_ptr().add(inline_shift as usize),
                data_va,
                copy_len,
            );
        }
        log::info!(
            "kabi: read_cache_folio: fake_page={:#x} data_va={:p} \
             mapping={:p} index={} path={} INLINE phys={:#x} \
             shift={} size={}",
            fake_page_va, data_va, mapping, index, path, physical_offset,
            inline_shift, copy_len,
        );
    }
    fake_page_va as *mut c_void
}

/// Phase 13: dispatch into mapping->a_ops->read_folio so the
/// filesystem's own code (e.g., `ext4_read_folio`) handles the
/// read.  Allocates a folio shadow entry for the caller to
/// receive, populates fake-page header (mapping, index, flags=0
/// since data isn't uptodate yet), then SCS-calls
/// `read_folio(file, folio)`.  ext4 fills the folio's data buffer
/// via `submit_bh` along the way.
///
/// Returns `Some(fake_page_va as ptr)` on success, `None` if
/// either `a_ops` or `a_ops->read_folio` is null (caller falls back
/// to the legacy raw-initramfs path).
fn dispatch_a_ops_read_folio(
    mapping: *mut c_void, file: *mut c_void, index: u64,
) -> Option<*mut c_void> {
    if mapping.is_null() {
        return None;
    }
    let a_ops: usize = unsafe {
        *(mapping.cast::<u8>()
            .add(super::struct_layouts::AS_A_OPS_OFF)
            as *const usize)
    };
    if a_ops == 0 {
        return None;
    }
    let read_folio_fn: usize = unsafe {
        *((a_ops + super::struct_layouts::AOPS_READ_FOLIO_OFF)
            as *const usize)
    };
    if read_folio_fn == 0 {
        return None;
    }
    // Don't dispatch into our own synth a_ops table (erofs's
    // mount-side mapping uses it); the legacy raw-initramfs fallback
    // handles those.
    let kabi_aops_table = super::fs_synth::kabi_aops_addr();
    if a_ops == kabi_aops_table {
        return None;
    }

    // Allocate a fresh folio (data area starts zero-filled; flags=0
    // because read_folio is responsible for populating + setting
    // PG_uptodate when complete).
    let (fake_page_va, _data_va) = super::folio_shadow::alloc_folio(
        0, // not yet uptodate
        mapping,
        index,
    )?;

    // Need a non-null `file` arg.  ext4_read_folio reads
    // file->f_inode->i_sb to get the superblock for bh submission.
    // If caller didn't pass one, allocate a transient file here.
    let (synth_file, owns_synth) = if file.is_null() {
        let f = super::alloc::kzalloc(
            super::struct_layouts::FILE_SIZE,
            super::alloc::__GFP_ZERO,
        );
        if f.is_null() {
            log::warn!(
                "kabi: read_cache_folio dispatch: kzalloc(FILE) failed"
            );
            return Some(err_ptr_eio());
        }
        // Set f_mapping = mapping, f_inode = mapping->host.
        let host: usize = unsafe {
            *(mapping.cast::<u8>()
                .add(super::struct_layouts::AS_HOST_OFF)
                as *const usize)
        };
        unsafe {
            *(f.cast::<u8>().add(super::struct_layouts::FILE_F_MAPPING_OFF)
                as *mut *mut c_void) = mapping;
            *(f.cast::<u8>().add(super::struct_layouts::FILE_F_INODE_OFF)
                as *mut usize) = host;
        }
        (f, true)
    } else {
        (file, false)
    };

    log::info!(
        "kabi: read_cache_folio: dispatching a_ops->read_folio \
         (mapping={:p} index={} fake_page={:#x})",
        mapping, index, fake_page_va,
    );
    let rc = super::loader::call_with_scs_2(
        read_folio_fn as *const (),
        synth_file as usize,
        fake_page_va as usize,
    ) as i32;
    if owns_synth {
        super::alloc::kfree(synth_file);
    }
    if rc < 0 {
        log::warn!(
            "kabi: read_cache_folio: a_ops->read_folio returned {}",
            rc,
        );
        return Some(err_ptr_eio());
    }
    Some(fake_page_va as *mut c_void)
}

/// Returns (backing_path, physical_byte_offset, inline_shift,
/// inline_size).  inline_shift > 0 means a FLAT_INLINE read where
/// the data needs to be relocated within the page; otherwise the
/// raw bytes at physical_offset are the page contents.
fn resolve_read_request(
    mapping: *mut c_void, file: *mut c_void, index: u64,
) -> (String, u64, u32, u64) {
    // Layout-aware: inspect mapping->host (the inode), look up
    // its KabiInodeMeta in the side-table.
    if !mapping.is_null() {
        let host: usize = unsafe {
            *(mapping.cast::<u8>()
                .add(super::struct_layouts::AS_HOST_OFF)
                as *const usize)
        };
        if host != 0 {
            if let Some(meta) = super::inode_meta::lookup_meta(host) {
                if let Some((path, phys, shift)) =
                    super::inode_meta::translate_offset(&meta, index)
                {
                    return (path, phys, shift, meta.i_size);
                }
            }
        }
    }
    // Fallback: raw read at `index * 4096`.  Used by the mount-time
    // superblock-read path where the backing file's f_mapping is
    // the global synth (host not registered in INODE_META).
    let path = super::fs_synth::lookup_synth_file_path_for_mapping(mapping, file)
        .unwrap_or_else(|| {
            log::warn!(
                "kabi: read_cache_folio: null mapping/file ({:p}/{:p}); \
                 falling back to /lib/test.erofs",
                mapping, file,
            );
            String::from("/lib/test.erofs")
        });
    (path, index * 4096, 0, 0)
}

/// Read up to `len` bytes from the initramfs file at `path`,
/// starting at byte `offset`, into the kernel buffer at `dst`.
pub fn read_initramfs_at(path: &str, offset: usize, dst: *mut u8, len: usize)
    -> Result<usize, kevlar_vfs::result::Error>
{
    use kevlar_vfs::file_system::FileSystem;
    use kevlar_vfs::user_buffer::UserBufferMut;
    use crate::fs::opened_file::OpenOptions;
    let initramfs = crate::fs::initramfs::INITRAM_FS.clone();
    let mut current = initramfs.root_dir()?;
    let mut iter = path.split('/').filter(|c| !c.is_empty()).peekable();
    while let Some(component) = iter.next() {
        let inode = current.lookup(component)?;
        if iter.peek().is_some() {
            current = inode.as_dir()?.clone();
        } else {
            let file = inode.as_file()?;
            let buf = unsafe { core::slice::from_raw_parts_mut(dst, len) };
            return file.read(offset, UserBufferMut::from(buf), &OpenOptions::readwrite());
        }
    }
    Err(kevlar_vfs::result::Error::new(kevlar_vfs::result::Errno::ENOENT))
}

#[unsafe(no_mangle)]
pub extern "C" fn readahead_expand(_ractl: *mut c_void, _new_start: u64,
                                   _new_len: usize) {}

#[unsafe(no_mangle)]
pub extern "C" fn page_cache_sync_ra(_ractl: *mut c_void, _req_count: usize) {}

ksym!(filemap_read);
ksym!(filemap_splice_read);
ksym!(read_cache_folio);
ksym!(readahead_expand);
ksym!(page_cache_sync_ra);

// ── invalidation / truncation ────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn truncate_inode_pages_final(_mapping: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn invalidate_mapping_pages(_mapping: *mut c_void,
                                           _start: u64,
                                           _end: u64) -> u64 {
    0
}

ksym!(truncate_inode_pages_final);
ksym!(invalidate_mapping_pages);
