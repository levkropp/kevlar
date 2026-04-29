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
pub extern "C" fn read_cache_folio(_mapping: *mut c_void, _index: u64,
                                   _filler: *const c_void,
                                   _file: *mut c_void) -> *mut c_void {
    log::warn!("kabi: read_cache_folio (stub) — ERR_PTR(-EIO)");
    err_ptr_eio()
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
