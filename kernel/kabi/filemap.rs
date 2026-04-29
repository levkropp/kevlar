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

#[unsafe(no_mangle)]
pub extern "C" fn filemap_read(_iocb: *mut c_void, _to: *mut c_void,
                               _already_read: isize) -> isize {
    log::warn!("kabi: filemap_read (stub)");
    0
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
    // Phase 3 v1: kmalloc a 4 KiB folio-shaped buffer in Kevlar VA
    // space + populate it with file data from the initramfs at
    // offset = index * 4096.
    //
    // Layout we hand back: the buffer IS the folio.  Linux folio
    // accessors expect:
    //   +0   unsigned long flags    — set bit 1 (PG_uptodate)
    //   +8   misc lru/mlock fields
    //   +16  struct address_space *mapping  — populate from arg
    //   +24  pgoff_t index           — populate from arg
    //   ...
    //
    // The actual file data lives starting at offset
    // `_data_off` inside the folio buffer.  Erofs reads it via
    // either:
    //   (a) `kmap_local_page(folio)` — inline VA arithmetic that
    //       in Linux's view returns the data buffer's VA.
    //   (b) Direct field reads on the folio struct.
    //
    // (b) works as long as we have flags + mapping + index sane.
    // (a) is the next blocker — erofs's compiled `kmap_local_page`
    // does `__va(__pfn_to_phys(page_to_pfn(page)))` arithmetic
    // that produces an address based on Linux's VMEMMAP_START.
    //
    // For now, the v1 implementation places the data starting at
    // offset 64 (skipping a small folio-header area) so direct
    // reads at small offsets see folio fields, but that's a
    // half-measure.  Phase 3b adds a real folio-→data mapping.
    let path = super::fs_synth::lookup_synth_file_path_for_mapping(mapping, file)
        .unwrap_or_else(|| {
            // Fallback: when erofs goes through sb->s_bdev->bd_mapping
            // (which we don't synthesise), the mapping arg is null.
            // Use the canonical Phase 3 test image until the bdev
            // path is wired through.
            log::warn!(
                "kabi: read_cache_folio: null mapping/file ({:p}/{:p}); \
                 falling back to /lib/test.erofs",
                mapping, file,
            );
            String::from("/lib/test.erofs")
        });

    let folio = super::alloc::kmalloc(4096, 0);
    if folio.is_null() {
        return err_ptr_eio();
    }
    unsafe { core::ptr::write_bytes(folio as *mut u8, 0, 4096); }

    // Set flags = PG_uptodate (bit 3 in Linux 7.0).  This signals
    // the folio's data is valid; callers that check uptodate skip
    // calling read_folio and use the data directly.
    const PG_UPTODATE_BIT: u64 = 1 << 3;
    unsafe {
        *(folio.cast::<u64>().add(0)) = PG_UPTODATE_BIT;
        // mapping at +16, index at +24
        *(folio.cast::<u8>().add(16) as *mut *mut c_void) = mapping;
        *(folio.cast::<u8>().add(24) as *mut u64) = index;
    }

    // Read file data at offset = index * 4096 into the folio,
    // starting at byte 0 of a backing data buffer.  We'd allocate
    // a separate data buffer for Phase 3b; for v1 we overwrite the
    // folio buffer past offset 64 (skipping the synthetic folio
    // header).
    let offset = (index * 4096) as usize;
    let data_start = unsafe { folio.cast::<u8>().add(64) };
    let to_read = 4096 - 64;
    if let Err(e) = read_initramfs_at(&path, offset, data_start, to_read) {
        log::warn!("kabi: read_cache_folio: initramfs read of {} @ {} failed: {:?}",
                   path, offset, e);
        super::alloc::kfree(folio);
        return err_ptr_eio();
    }
    log::info!(
        "kabi: read_cache_folio: folio={:p} mapping={:p} index={} \
         path={} data@+64 (4032 bytes)",
        folio, mapping, index, path,
    );
    folio
}

/// Read up to `len` bytes from the initramfs file at `path`,
/// starting at byte `offset`, into the kernel buffer at `dst`.
fn read_initramfs_at(path: &str, offset: usize, dst: *mut u8, len: usize)
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
