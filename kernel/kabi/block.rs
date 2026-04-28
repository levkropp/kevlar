// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux block-layer stubs (K33 Phase 2).
//!
//! Targeted at filesystem .ko modules (ext4, erofs, etc.) — the
//! handful of block primitives a fs needs to read/write a backing
//! device:
//!
//!   * `bdev_file_open_by_path` / `file_bdev` / `fput` — open the
//!     `/dev/sdaN` file backing `mount -t ext4 /dev/sdaN /mnt`,
//!     unwrap the `block_device *` from the file, eventually
//!     `fput()` it.
//!   * `submit_bio` / `bio_init` / `bio_endio` / `bio_alloc_bioset`
//!     / `bio_add_page` / `bio_add_folio` / `bio_put` /
//!     `bio_uninit` — synchronous bio routing to `exts/virtio_blk`.
//!   * `__bread` / `__getblk` / `sb_bread` / `sb_getblk` — buffer
//!     head reads (ext2/4 metadata path).
//!   * `bdev_get_queue` / `bdi_dev_name` / `super_setup_bdi` —
//!     trivial stubs; we don't model Linux's request_queue.
//!
//! K33 scope is **bring-up scaffolding**.  Each fn is currently a
//! `log::warn!` stub returning `-ENOSYS`-ish defaults.  As we
//! attempt to load each filesystem .ko, the loader's
//! "unresolved symbol" panic tells us which function the module
//! actually calls; we then fill in a real (or close-enough) impl.
//!
//! Tracking: 241 undefined symbols across erofs.ko (the simpler
//! pivot from ext4 since Ubuntu builds ext4 builtin and erofs is
//! a .ko in the modules deb).  About 30 of those are block-layer
//! and live here; the rest split between filemap.rs / fs_register.rs
//! / jbd2_stubs.rs / existing kABI modules.

use core::ffi::{c_int, c_void};

use crate::ksym;

// ── bio ──────────────────────────────────────────────────────────
// Linux's bio is the request descriptor for block I/O.  Filesystem
// modules build a bio (initialise → add pages/folios → submit_bio
// → wait on completion).  K33 v1 implementation: synchronous
// pass-through to `exts/virtio_blk` reads/writes — there's no real
// queueing because we don't have a multi-queue block layer yet.

#[unsafe(no_mangle)]
pub extern "C" fn bio_init(_bio: *mut c_void, _bdev: *mut c_void,
                           _table: *mut c_void, _max_vecs: u16,
                           _opf: u32) {
    log::warn!("kabi: bio_init (stub)");
}

#[unsafe(no_mangle)]
pub extern "C" fn bio_alloc_bioset(_bdev: *mut c_void, _nr_vecs: u16,
                                   _opf: u32, _gfp: u32,
                                   _bs: *mut c_void) -> *mut c_void {
    log::warn!("kabi: bio_alloc_bioset (stub) — returning null");
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn bio_add_page(_bio: *mut c_void, _page: *mut c_void,
                               _len: u32, _off: u32) -> c_int {
    log::warn!("kabi: bio_add_page (stub)");
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn bio_add_folio(_bio: *mut c_void, _folio: *mut c_void,
                                _len: usize, _off: usize) -> bool {
    log::warn!("kabi: bio_add_folio (stub)");
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn bio_endio(_bio: *mut c_void) {
    log::warn!("kabi: bio_endio (stub)");
}

#[unsafe(no_mangle)]
pub extern "C" fn bio_put(_bio: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn bio_uninit(_bio: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn submit_bio(_bio: *mut c_void) {
    log::warn!("kabi: submit_bio (stub) — bio not actually issued");
}

#[unsafe(no_mangle)]
pub extern "C" fn errno_to_blk_status(_errno: c_int) -> u8 {
    0
}

ksym!(bio_init);
ksym!(bio_alloc_bioset);
ksym!(bio_add_page);
ksym!(bio_add_folio);
ksym!(bio_endio);
ksym!(bio_put);
ksym!(bio_uninit);
ksym!(submit_bio);
ksym!(errno_to_blk_status);

// ── block_device / bdev_file ─────────────────────────────────────
// Linux passes the backing block device as a `struct file *`
// returned by `bdev_file_open_by_path("/dev/sdaN", ...)`.  The fs
// then unwraps the `block_device *` via `file_bdev()` and uses it
// as the backing handle for all subsequent bios.  v1 stub returns
// null pointers; the fs registry's `kabi_mount_filesystem` will
// eventually inject a synthetic `block_device` wrapping our
// `exts/virtio_blk` driver.

#[unsafe(no_mangle)]
pub extern "C" fn bdev_file_open_by_path(_path: *const u8, _mode: u32,
                                         _holder: *mut c_void,
                                         _hops: *const c_void) -> *mut c_void {
    log::warn!("kabi: bdev_file_open_by_path (stub) — ERR_PTR(-ENODEV)");
    // Linux convention: error returns from pointer-typed functions are
    // encoded as `(void *)(-errno)` where IS_ERR(ptr) checks the top
    // bits.  Returning null would survive the IS_ERR check (null is
    // not an error to Linux) and be dereferenced by the caller.
    // -ENODEV = -19 → cast to *mut c_void gives a high-bits-set
    // pointer that Linux's IS_ERR catches.
    err_ptr(-19)
}

/// Encode an errno as Linux's `ERR_PTR(-errno)` pointer.  Linux's
/// `IS_ERR(ptr)` is `(unsigned long)(ptr) >= (unsigned long)-MAX_ERRNO`,
/// where MAX_ERRNO = 4095.  Negative-cast errno values fit naturally.
#[inline(always)]
pub(super) fn err_ptr(errno: isize) -> *mut c_void {
    errno as *mut c_void
}

#[unsafe(no_mangle)]
pub extern "C" fn file_bdev(_file: *mut c_void) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn bdev_get_queue(_bdev: *mut c_void) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn bdi_dev_name(_bdi: *mut c_void) -> *const u8 {
    b"kabi-bdi\0".as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn super_setup_bdi(_sb: *mut c_void) -> c_int {
    0
}

ksym!(bdev_file_open_by_path);
ksym!(file_bdev);
ksym!(bdev_get_queue);
ksym!(bdi_dev_name);
ksym!(super_setup_bdi);

// ── buffer head reads ────────────────────────────────────────────
// Filesystem metadata paths (especially ext2/4) read superblock
// + group-descriptor + bitmap blocks through the buffer-head API.
// `__bread(bdev, block, size)` returns a `struct buffer_head *`
// holding a kernel buffer with the block's contents.  v1 stub
// returns null — first fs that hits this gets the real impl.

#[unsafe(no_mangle)]
pub extern "C" fn __bread(_bdev: *mut c_void, _block: u64,
                          _size: u32) -> *mut c_void {
    log::warn!("kabi: __bread (stub) — returning null");
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn __getblk(_bdev: *mut c_void, _block: u64,
                           _size: u32) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_bread(_sb: *mut c_void, _block: u64) -> *mut c_void {
    log::warn!("kabi: sb_bread (stub) — returning null");
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_getblk(_sb: *mut c_void, _block: u64) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_set_blocksize(_sb: *mut c_void, _size: c_int) -> c_int {
    0
}

ksym!(__bread);
ksym!(__getblk);
ksym!(sb_bread);
ksym!(sb_getblk);
ksym!(sb_set_blocksize);
