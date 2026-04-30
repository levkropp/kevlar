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
use super::struct_layouts as fl;

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

// Phase 13: minimal bio infrastructure.  ext4_mpage_readpages allocates
// a bio, sets bi_iter.bi_sector, calls bio_add_folio for each folio,
// then submit_bio.  We synth a bio in a single 320-byte allocation:
//
//   +0..+144   Linux struct bio fields (we care about bi_bdev +8,
//              bi_io_vec +40, bi_iter.bi_sector +48, bi_iter.bi_size
//              +56, bi_end_io +80, bi_private +88).
//   +144       u16 our_bvec_count                  (kABI scratch)
//   +152..+280 BioVec[8] inline storage            (16 bytes each).
//
// bio_alloc_bioset zeros, sets bi_bdev, points bi_io_vec at the
// inline storage.  bio_add_folio appends a vec.  submit_bio reads
// from the bdev synchronously into each folio's data buffer (using
// folio_to_data_va to compute the kernel VA), marks each folio
// PG_uptodate, then invokes bi_end_io.

const BIO_TOTAL_SIZE: usize = 320;
const BIO_BVEC_INLINE_OFF: usize = 192;
const BIO_BVEC_COUNT_OFF: usize = 184;
const MAX_INLINE_BVECS: usize = 8;

// Ubuntu's compiled ext4.ko bio layout (verified via disasm of
// `ext4_mpage_readpages` around the bio_alloc_bioset call: writes
// `bi_iter.bi_sector` to bio[+40] and `bi_end_io` to bio[+64]).
// This is 8 bytes shorter before bi_iter than mainline's struct bio
// — likely because Ubuntu's CONFIG omits the __bi_remaining +
// padding pair that vanilla 7.0 has between the small-int header
// and bi_io_vec.
const BIO_BI_BDEV_OFF: usize = 8;
const BIO_BI_IO_VEC_OFF: usize = 32;
const BIO_BI_SECTOR_OFF: usize = 40;
const BIO_BI_SIZE_OFF: usize = 48;
const BIO_BI_END_IO_OFF: usize = 64;
const BIO_BI_PRIVATE_OFF: usize = 72;

const BVEC_BV_PAGE_OFF: usize = 0;
const BVEC_BV_LEN_OFF: usize = 8;
const BVEC_BV_OFFSET_OFF: usize = 12;
const BVEC_SIZE: usize = 16;

const PG_UPTODATE_FLAG: u64 = 1 << 3;

#[unsafe(no_mangle)]
pub extern "C" fn bio_alloc_bioset(bdev: *mut c_void, _nr_vecs: u16,
                                   _opf: u32, _gfp: u32,
                                   _bs: *mut c_void) -> *mut c_void {
    let bio = super::alloc::kzalloc(BIO_TOTAL_SIZE, super::alloc::__GFP_ZERO);
    if bio.is_null() { return core::ptr::null_mut(); }
    unsafe {
        // bi_bdev
        *(bio.cast::<u8>().add(BIO_BI_BDEV_OFF) as *mut *mut c_void) = bdev;
        // bi_io_vec → inline bvec storage
        *(bio.cast::<u8>().add(BIO_BI_IO_VEC_OFF) as *mut *mut u8)
            = bio.cast::<u8>().add(BIO_BVEC_INLINE_OFF);
    }
    bio
}

#[unsafe(no_mangle)]
pub extern "C" fn bio_add_page(_bio: *mut c_void, _page: *mut c_void,
                               _len: u32, _off: u32) -> c_int {
    log::warn!("kabi: bio_add_page (stub) — folio path is the only one used");
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn bio_add_folio(bio: *mut c_void, folio: *mut c_void,
                                len: usize, off: usize) -> bool {
    if bio.is_null() || folio.is_null() { return false; }
    unsafe {
        let count = *(bio.cast::<u8>().add(BIO_BVEC_COUNT_OFF) as *const u16);
        if count as usize >= MAX_INLINE_BVECS {
            return false;
        }
        let bvec = bio.cast::<u8>()
            .add(BIO_BVEC_INLINE_OFF + count as usize * BVEC_SIZE);
        *(bvec.add(BVEC_BV_PAGE_OFF) as *mut *mut c_void) = folio;
        *(bvec.add(BVEC_BV_LEN_OFF) as *mut u32) = len as u32;
        *(bvec.add(BVEC_BV_OFFSET_OFF) as *mut u32) = off as u32;
        *(bio.cast::<u8>().add(BIO_BVEC_COUNT_OFF) as *mut u16) = count + 1;
        // Advance bi_iter.bi_size by `len`.
        let cur_size = *(bio.cast::<u8>().add(BIO_BI_SIZE_OFF) as *const u32);
        *(bio.cast::<u8>().add(BIO_BI_SIZE_OFF) as *mut u32)
            = cur_size + len as u32;
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn bio_endio(bio: *mut c_void) {
    if bio.is_null() { return; }
    unsafe {
        let end_io: usize = *(bio.cast::<u8>().add(BIO_BI_END_IO_OFF)
            as *const usize);
        if end_io != 0 {
            type EndIoFn = unsafe extern "C" fn(*mut c_void);
            let f: EndIoFn = core::mem::transmute(end_io);
            f(bio);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn bio_put(bio: *mut c_void) {
    if !bio.is_null() {
        super::alloc::kfree(bio);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn bio_uninit(_bio: *mut c_void) {}

/// Real `submit_bio`: walks bi_io_vec, reads from the registered
/// BlockDevice into each folio's data buffer (computed via
/// `folio_to_data_va`), marks each folio PG_uptodate, then calls
/// `bi_end_io` if registered.  For our synchronous shim the read
/// has already completed by the time we return.
#[unsafe(no_mangle)]
pub extern "C" fn submit_bio(bio: *mut c_void) {
    if bio.is_null() { return; }
    let device = match kevlar_api::driver::block::block_device() {
        Some(d) => d,
        None => {
            log::warn!("kabi: submit_bio: no BlockDevice registered");
            return;
        }
    };
    let sector_size = device.sector_size() as u64;
    unsafe {
        let mut current_sector: u64 = *(bio.cast::<u8>()
            .add(BIO_BI_SECTOR_OFF) as *const u64);
        let count = *(bio.cast::<u8>().add(BIO_BVEC_COUNT_OFF) as *const u16);
        log::info!(
            "kabi: submit_bio: bio={:p} start_sector={} bvec_count={}",
            bio, current_sector, count,
        );
        for i in 0..count as usize {
            let bvec = bio.cast::<u8>()
                .add(BIO_BVEC_INLINE_OFF + i * BVEC_SIZE);
            let folio: *mut c_void = *(bvec.add(BVEC_BV_PAGE_OFF)
                as *const *mut c_void);
            let len: u32 = *(bvec.add(BVEC_BV_LEN_OFF) as *const u32);
            let off: u32 = *(bvec.add(BVEC_BV_OFFSET_OFF) as *const u32);
            if folio.is_null() || len == 0 { continue; }

            let data_va = super::folio_shadow::folio_to_data_va(folio as u64);
            let dst = core::slice::from_raw_parts_mut(
                data_va.add(off as usize), len as usize,
            );
            log::info!(
                "kabi: submit_bio: bvec[{}] folio={:p} data_va={:p} \
                 len={} off={} sector={}",
                i, folio, data_va, len, off, current_sector,
            );
            if let Err(e) = device.read_sectors(current_sector, dst) {
                log::warn!(
                    "kabi: submit_bio: read_sectors at {} failed: {:?}",
                    current_sector, e,
                );
                return;
            }
            // Mark folio uptodate (folio->flags at offset 0, bit 3).
            let flags_ptr = folio.cast::<u8>() as *mut u64;
            *flags_ptr = *flags_ptr | PG_UPTODATE_FLAG;
            current_sector += (len as u64 + sector_size - 1) / sector_size;
        }
        // Invoke bi_end_io if registered (e.g. mpage's bio_endio handler).
        let end_io: usize = *(bio.cast::<u8>().add(BIO_BI_END_IO_OFF)
            as *const usize);
        if end_io != 0 {
            type EndIoFn = unsafe extern "C" fn(*mut c_void);
            let f: EndIoFn = core::mem::transmute(end_io);
            f(bio);
        } else {
            // No end_io callback — free the bio ourselves.
            super::alloc::kfree(bio);
        }
    }
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

/// Phase 11: real `bdev_file_open_by_path`.  Allocate a synth
/// `struct file` (256 B) + `struct inode` (1024 B) + `struct
/// block_device` (256 B) trio.  The block_device pointer is the
/// handle ext4's mount path passes around (via `file_bdev`,
/// `sb->s_bdev`).  Block reads route through the registered
/// `kevlar_api::driver::block::block_device()` directly — single-
/// device v1.
#[unsafe(no_mangle)]
pub extern "C" fn bdev_file_open_by_path(path: *const u8, _mode: u32,
                                         _holder: *mut c_void,
                                         _hops: *const c_void) -> *mut c_void {
    let path_str = if !path.is_null() {
        let len = unsafe {
            let mut n = 0usize;
            while *path.add(n) != 0 && n < 64 { n += 1; }
            n
        };
        let bytes = unsafe { core::slice::from_raw_parts(path, len) };
        core::str::from_utf8(bytes).unwrap_or("<non-utf8>")
    } else { "<null>" };

    if kevlar_api::driver::block::block_device().is_none() {
        log::warn!("kabi: bdev_file_open_by_path({:?}) — no BlockDevice registered",
                   path_str);
        return err_ptr(-19); // -ENODEV
    }

    let file = super::alloc::kmalloc(fl::FILE_SIZE, super::alloc::__GFP_ZERO);
    let inode = super::alloc::kmalloc(fl::INODE_SIZE, super::alloc::__GFP_ZERO);
    let bdev = super::alloc::kmalloc(fl::SB_SIZE, super::alloc::__GFP_ZERO);
    let host_sb = super::alloc::kmalloc(fl::SB_SIZE, super::alloc::__GFP_ZERO);
    if file.is_null() || inode.is_null() || bdev.is_null() || host_sb.is_null() {
        log::warn!("kabi: bdev_file_open_by_path: kmalloc failed");
        return err_ptr(-12); // -ENOMEM
    }

    unsafe {
        // file->f_inode = inode
        *(file.cast::<u8>().add(fl::FILE_F_INODE_OFF) as *mut *mut c_void) = inode;
        // inode->i_sb = host_sb (Phase 7 fix — avoid NULL deref on
        // i_sb->s_op under user page tables).
        *(inode.cast::<u8>().add(fl::INODE_I_SB_OFF) as *mut *mut c_void) = host_sb;
        // inode->i_mode = S_IFBLK | 0666
        const S_IFBLK: u16 = 0o060000;
        *(inode.cast::<u8>().add(fl::INODE_I_MODE_OFF) as *mut u16)
            = S_IFBLK | 0o666;
        // inode->i_blkbits = 9 (512-byte sectors)
        *(inode.cast::<u8>().add(fl::INODE_I_BLKBITS_OFF) as *mut u8) = 9;
    }

    // Stash the bdev pointer in a side-table keyed by `file`.  ext4's
    // file_bdev() reads this back.
    SYNTH_BDEV_TABLE.lock().push(SynthBdevEntry {
        file_ptr: file as usize,
        bdev_ptr: bdev as usize,
    });

    log::info!("kabi: bdev_file_open_by_path({:?}) → file={:p} bdev={:p}",
               path_str, file, bdev);
    file
}

#[derive(Clone, Copy)]
struct SynthBdevEntry {
    file_ptr: usize,
    bdev_ptr: usize,
}

static SYNTH_BDEV_TABLE: kevlar_platform::spinlock::SpinLock<
    alloc::vec::Vec<SynthBdevEntry>,
> = kevlar_platform::spinlock::SpinLock::new(alloc::vec::Vec::new());

/// Encode an errno as Linux's `ERR_PTR(-errno)` pointer.  Linux's
/// `IS_ERR(ptr)` is `(unsigned long)(ptr) >= (unsigned long)-MAX_ERRNO`,
/// where MAX_ERRNO = 4095.  Negative-cast errno values fit naturally.
#[inline(always)]
pub(super) fn err_ptr(errno: isize) -> *mut c_void {
    errno as *mut c_void
}

/// Phase 11: real `file_bdev` — look up the synth bdev pointer
/// allocated by `bdev_file_open_by_path` for the given file.
#[unsafe(no_mangle)]
pub extern "C" fn file_bdev(file: *mut c_void) -> *mut c_void {
    let table = SYNTH_BDEV_TABLE.lock();
    for entry in table.iter() {
        if entry.file_ptr == file as usize {
            return entry.bdev_ptr as *mut c_void;
        }
    }
    log::warn!("kabi: file_bdev({:p}) — not in side-table", file);
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
pub extern "C" fn super_setup_bdi(sb: *mut c_void) -> c_int {
    log::warn!("kabi: super_setup_bdi(sb={:p}) called", sb);
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

/// Phase 11: real `__bread`.  Read `size` bytes at block `block` from
/// the registered virtio_blk device, allocate a `struct buffer_head`
/// + 4 KiB data buffer, populate fields, return.  ext4/jbd2's
/// metadata reads (superblock, BGDT, inode tables) all flow here.
#[unsafe(no_mangle)]
pub extern "C" fn __bread(bdev: *mut c_void, block: u64,
                          size: u32) -> *mut c_void {
    let device = match kevlar_api::driver::block::block_device() {
        Some(d) => d,
        None => {
            log::warn!("kabi: __bread: no BlockDevice registered");
            return core::ptr::null_mut();
        }
    };
    let sector_size = device.sector_size() as u64;
    if sector_size == 0 || (size as u64) % sector_size != 0 {
        log::warn!("kabi: __bread: size {} not a multiple of sector_size {}",
                   size, sector_size);
        return core::ptr::null_mut();
    }
    let start_sector = block * (size as u64) / sector_size;

    // Allocate buffer_head + data buffer.
    let bh = super::alloc::kmalloc(fl::BH_SIZE, super::alloc::__GFP_ZERO);
    if bh.is_null() {
        return core::ptr::null_mut();
    }
    let data_void = super::alloc::kmalloc(size as usize, super::alloc::__GFP_ZERO);
    if data_void.is_null() {
        super::alloc::kfree(bh);
        return core::ptr::null_mut();
    }
    let data = data_void as *mut u8;

    // Issue the read.
    let buf = unsafe { core::slice::from_raw_parts_mut(data, size as usize) };
    if let Err(e) = device.read_sectors(start_sector, buf) {
        log::warn!("kabi: __bread: read_sectors(start={}, len={}) failed: {:?}",
                   start_sector, size, e);
        super::alloc::kfree(data_void);
        super::alloc::kfree(bh);
        return core::ptr::null_mut();
    }

    // Populate buffer_head fields.
    unsafe {
        *(bh.cast::<u8>().add(fl::BH_B_STATE_OFF) as *mut u64) = fl::BH_UPTODATE;
        *(bh.cast::<u8>().add(fl::BH_B_BLOCKNR_OFF) as *mut u64) = block;
        *(bh.cast::<u8>().add(fl::BH_B_SIZE_OFF) as *mut u64) = size as u64;
        *(bh.cast::<u8>().add(fl::BH_B_DATA_OFF) as *mut *mut u8) = data;
        *(bh.cast::<u8>().add(fl::BH_B_BDEV_OFF) as *mut *mut c_void) = bdev;
    }

    log::info!("kabi: __bread(block={}, size={}) → bh={:p} data={:p}",
               block, size, bh, data);
    bh
}

#[unsafe(no_mangle)]
pub extern "C" fn __getblk(bdev: *mut c_void, block: u64,
                           size: u32) -> *mut c_void {
    // For RO mount __getblk is rarely distinct from __bread; just
    // return a buffer_head we can read into later if needed.
    __bread(bdev, block, size)
}

/// Phase 11: real `sb_bread` — derive blocksize + bdev from sb,
/// dispatch to `__bread`.
#[unsafe(no_mangle)]
pub extern "C" fn sb_bread(sb: *mut c_void, block: u64) -> *mut c_void {
    if sb.is_null() {
        return core::ptr::null_mut();
    }
    let blocksize = unsafe {
        *(sb.cast::<u8>().add(fl::SB_S_BLOCKSIZE_OFF) as *const u64) as u32
    };
    let bdev = unsafe {
        *(sb.cast::<u8>().add(fl::SB_S_BDEV_OFF) as *const *mut c_void)
    };
    if blocksize == 0 {
        log::warn!("kabi: sb_bread: sb->s_blocksize == 0");
        return core::ptr::null_mut();
    }
    __bread(bdev, block, blocksize)
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_getblk(sb: *mut c_void, block: u64) -> *mut c_void {
    sb_bread(sb, block)
}

/// Phase 11: real `sb_set_blocksize` — write the new blocksize +
/// blocksize_bits into sb.  Returns the new size on success, 0 on
/// failure.
#[unsafe(no_mangle)]
pub extern "C" fn sb_set_blocksize(sb: *mut c_void, size: c_int) -> c_int {
    if sb.is_null() || size < 512 || (size as u32 & (size as u32 - 1)) != 0 {
        return 0;
    }
    let bits = (size as u32).trailing_zeros() as u8;
    unsafe {
        *(sb.cast::<u8>().add(fl::SB_S_BLOCKSIZE_BITS_OFF) as *mut u8) = bits;
        *(sb.cast::<u8>().add(fl::SB_S_BLOCKSIZE_OFF) as *mut u64) = size as u64;
    }
    size
}

ksym!(__bread);
ksym!(__getblk);
ksym!(sb_bread);
ksym!(sb_getblk);
ksym!(sb_set_blocksize);
