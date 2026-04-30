// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Bulk no-op stubs for the long tail of fs/inode/iomap/dax/
//! crypto/compression/xarray symbols that filesystem .ko modules
//! reference but rarely actually call on a successful RO mount.
//!
//! Strategy: each function takes pointers/integers and returns
//! null / 0 / -ENOSYS-ish.  When we attempt a real mount that
//! actually calls one of these, the boot crashes or returns
//! -ENOENT and we know to write a real impl.  Until then, every
//! stub here exists purely to satisfy the loader's symbol
//! resolution.

use core::ffi::{c_int, c_void};

use crate::ksym;

// ── VFS / inode ──────────────────────────────────────────────────
//
// Phase 4c-4e: real allocation for inode + dentry.  Each kmalloc'd,
// zero-filled, with the few fields erofs reads/writes populated.
// Field offsets verified via struct_layouts.rs.

use super::struct_layouts as fl;

/// Walk `sb->s_fs_info->dif0.file->f_mapping` to find the per-mount
/// synth address_space pointer.  Phase 5 v2: every kABI-allocated
/// inode points its `i_mapping` at this so erofs's directory + file
/// read paths (which set `buf.mapping = inode->i_mapping`) get a
/// mapping that's registered in our SYNTH_FILES table — letting
/// `read_cache_folio` look up the backing initramfs path.
///
/// Returns null if the chain isn't yet populated (e.g. first
/// allocations during init_fs_context, before sbi->dif0.file is set).
fn synth_mapping_from_sb(sb: *mut c_void) -> *mut c_void {
    if sb.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        let sbi = *((sb as *const u8)
            .add(fl::SB_S_FS_INFO_OFF) as *const *mut c_void);
        if sbi.is_null() {
            return core::ptr::null_mut();
        }
        // sbi->dif0.file at offset 16 (struct erofs_device_info:
        // .path +0, .fscache +8, .file +16).  Verified via Phase 4g
        // sbi dump (file pointer at sbi+0x10).
        let file = *((sbi as *const u8).add(16) as *const *mut c_void);
        if file.is_null() {
            return core::ptr::null_mut();
        }
        // file->f_mapping at +16 (FILE_F_MAPPING_OFF).
        *((file as *const u8).add(fl::FILE_F_MAPPING_OFF)
            as *const *mut c_void)
    }
}

/// `d_make_root(inode)` — Linux's "make a root dentry pointing at
/// `inode`".  We allocate a `DENTRY_SIZE`-byte dentry, link it to
/// the inode, and set the parent to itself (root convention).
#[unsafe(no_mangle)]
pub extern "C" fn d_make_root(inode: *mut c_void) -> *mut c_void {
    if inode.is_null() {
        log::warn!("kabi: d_make_root: inode is null");
        return core::ptr::null_mut();
    }
    let dentry = super::alloc::kmalloc(fl::DENTRY_SIZE, 0);
    if dentry.is_null() {
        log::warn!("kabi: d_make_root: kmalloc failed");
        return core::ptr::null_mut();
    }
    unsafe { core::ptr::write_bytes(dentry as *mut u8, 0, fl::DENTRY_SIZE); }
    // d_inode at +56, d_parent at +32 (self-pointer for root).
    // d_flags = DCACHE_DIRECTORY_TYPE (0x00200000) at +0.
    const DCACHE_DIRECTORY_TYPE: u32 = 0x0020_0000;
    unsafe {
        *(dentry.cast::<u8>().add(fl::DENTRY_D_FLAGS_OFF) as *mut u32) =
            DCACHE_DIRECTORY_TYPE;
        *(dentry.cast::<u8>().add(fl::DENTRY_D_PARENT_OFF) as *mut *mut c_void) =
            dentry;
        *(dentry.cast::<u8>().add(fl::DENTRY_D_INODE_OFF) as *mut *mut c_void) =
            inode;
    }
    log::info!("kabi: d_make_root: dentry={:p} inode={:p}", dentry, inode);
    dentry
}

#[unsafe(no_mangle)]
pub extern "C" fn d_obtain_alias(_inode: *mut c_void) -> *mut c_void {
    core::ptr::null_mut()
}

/// `d_splice_alias(inode, dentry)` — Linux's helper that
/// connects a freshly-allocated dentry to a looked-up inode.
///
/// Real-Linux behavior:
///   * `inode == NULL`                         → negative dentry; return NULL.
///   * `IS_ERR(inode)` (top byte is -errno)    → forward the error pointer.
///   * Existing alias for inode                → return the alias dentry.
///   * Otherwise                               → set dentry->d_inode = inode;
///                                                return NULL ("use input").
///
/// Phase 5 v4: minimal impl covering the cases erofs's
/// `erofs_lookup` produces — set `d_inode` on the input dentry,
/// or forward error/NULL.  No alias-table walk; we don't have one.
#[unsafe(no_mangle)]
pub extern "C" fn d_splice_alias(inode: *mut c_void,
                                 dentry: *mut c_void) -> *mut c_void {
    if inode.is_null() {
        // Negative dentry — name not found.  Caller checks
        // dentry->d_inode == NULL.
        return core::ptr::null_mut();
    }
    let v = inode as isize;
    if v >= -4095 && v < 0 {
        // ERR_PTR — forward.
        return inode;
    }
    if dentry.is_null() {
        log::warn!("kabi: d_splice_alias: null dentry");
        return core::ptr::null_mut();
    }
    // Positive: set dentry->d_inode = inode; return NULL.
    unsafe {
        *(dentry.cast::<u8>().add(fl::DENTRY_D_INODE_OFF)
            as *mut *mut c_void) = inode;
    }
    log::info!("kabi: d_splice_alias: dentry={:p} ← inode={:p}",
               dentry, inode);
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub static dotdot_name: [u8; 32] = *b"..\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0";

#[unsafe(no_mangle)]
pub extern "C" fn iget_failed(_inode: *mut c_void) {}

/// `iget5_locked(sb, hashval, test, set, data)` — Linux's
/// "find-or-allocate inode by 64-bit key" helper.  We don't have
/// a real inode hash, so each call allocates a fresh inode and
/// runs the `set` callback.  Returns NULL on alloc failure
/// (Linux's contract — caller IS_ERR-checks separately).
///
/// The `set` callback signature is `int (*)(struct inode *, void *)`
/// where the second arg is `data`.  Erofs uses it (`erofs_iget5_set`
/// at 0x5068) to store the inode number passed in `data`.
///
/// Phase 5 v3: hashval doubles as the erofs nid (per
/// erofs_iget at 0x5edc which loads `lsr x5, x1, #63` — the hashval
/// arg).  We use `hashval >> 1` (since erofs encodes nid in the
/// bottom 31 bits + a flag in bit 32+) to derive the on-disk inode
/// location.  Fallback: for v3 we trust `data` which is a `*const u64`
/// holding the raw nid set by erofs's caller.
#[unsafe(no_mangle)]
pub extern "C" fn iget5_locked(sb: *mut c_void, hashval: u64,
                               _test: *mut c_void, set: *mut c_void,
                               data: *mut c_void) -> *mut c_void {
    let inode = super::alloc::kmalloc(fl::INODE_SIZE, 0);
    if inode.is_null() {
        log::warn!("kabi: iget5_locked: kmalloc failed");
        return core::ptr::null_mut();
    }
    unsafe { core::ptr::write_bytes(inode as *mut u8, 0, fl::INODE_SIZE); }
    // Set i_sb; rest of the fields are populated by erofs's set
    // callback + erofs_read_inode after iget5 returns.
    unsafe {
        *(inode.cast::<u8>().add(fl::INODE_I_SB_OFF) as *mut *mut c_void) = sb;
    }
    // Phase 5 v3: allocate a PER-INODE address_space so that
    // `mapping->host` uniquely identifies this inode.  Our
    // `read_cache_folio` reads `mapping->host` to look up
    // KabiInodeMeta in INODE_META and translate logical-page-N
    // to physical disk offset.  The a_ops field stays empty (our
    // read_cache_folio doesn't dispatch through it; it does its
    // own translation).
    let per_inode_mapping = super::alloc::kzalloc(fl::AS_SIZE,
        super::alloc::__GFP_ZERO);
    if !per_inode_mapping.is_null() {
        unsafe {
            *(per_inode_mapping.cast::<u8>().add(fl::AS_HOST_OFF)
                as *mut *mut c_void) = inode;
        }
    }
    let mapping = if per_inode_mapping.is_null() {
        // Fall back to the global synth mapping if alloc fails.
        synth_mapping_from_sb(sb)
    } else {
        per_inode_mapping
    };
    unsafe {
        *(inode.cast::<u8>().add(fl::INODE_I_MAPPING_OFF)
            as *mut *mut c_void) = mapping;
    }
    // Mark I_NEW (bit 0 of i_state at +144 in Linux 7.0).  Linux's
    // enum sets `__I_NEW = 0U`, so `I_NEW = 1 << 0 = 1`.  Erofs
    // checks this bit in `erofs_iget` (at offset 0x5f30) to decide
    // whether to call `erofs_read_inode` to populate fields from
    // disk.  Without this set, erofs returns the bare inode with
    // i_mode=0 → fc_fill_super's S_IFDIR check fails → -EINVAL.
    const I_NEW: u32 = 1 << 0;
    unsafe {
        *(inode.cast::<u8>().add(144) as *mut u32) = I_NEW;
    }
    // i_blkbits — Linux's `inode_init_always` sets this from
    // `sb->s_blocksize_bits` (= 12 for our test image).  Erofs's
    // `find_target_block_classic` reads `dir->[+134]` to compute
    // `iblks = round_up(i_size, 1 << blkbits) >> blkbits`; with
    // blkbits=0 the binary search probes mid = (i_size-1)/2 = 33
    // blocks past EOF for a 68-byte root dir, returning
    // -EFSCORRUPTED.
    unsafe {
        *(inode.cast::<u8>().add(fl::INODE_I_BLKBITS_OFF) as *mut u8) = 12;
    }
    // Run the set callback if non-null — erofs uses it to store
    // the inode number.
    if !set.is_null() {
        type SetFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32;
        let set_fn: SetFn = unsafe { core::mem::transmute(set) };
        // Set callbacks have the same SCS hand-off requirement.
        let rc = super::loader::call_with_scs_2(
            set as *const (), inode as usize, data as usize,
        );
        log::info!("kabi: iget5_locked: set_fn returned {}", rc);
        let _ = set_fn; // keep type alias for documentation
    }

    // Phase 5 v3: register this inode's metadata in the side-table.
    // The nid is in `data` (per erofs_iget5_set which dereferences
    // `*data` as `erofs_nid_t`); on the first call (root inode),
    // `data` points at a u64 = sbi->root_nid.
    //
    // Backing path: derived from sb's s_fs_info.dif0.file via the
    // synth file lookup (filp_open_synth registered the path).
    let nid = if !data.is_null() {
        unsafe { *(data as *const u64) }
    } else {
        // Fallback — derive from hashval (erofs_iget passes hashval
        // = encoded nid; the low bits are the actual nid).
        hashval & 0x7fff_ffff
    };
    let backing_path = synth_backing_path(sb);
    if let Some(path) = backing_path {
        let _ = super::inode_meta::register_inode_from_nid(
            inode as usize, nid, &path,
        );
    } else {
        log::warn!("kabi: iget5_locked: no backing path for sb={:p}", sb);
    }

    log::info!("kabi: iget5_locked: inode={:p} sb={:p} data={:p} nid={}",
               inode, sb, data, nid);
    inode
}

/// Resolve the backing-file path for a kABI-mounted sb by looking
/// up `sb->s_fs_info->dif0.file` in the SYNTH_FILES table.
fn synth_backing_path(sb: *mut c_void) -> Option<alloc::string::String> {
    if sb.is_null() {
        return None;
    }
    let file_ptr = unsafe {
        let sbi = *((sb as *const u8)
            .add(fl::SB_S_FS_INFO_OFF) as *const *mut c_void);
        if sbi.is_null() {
            return None;
        }
        // sbi->dif0.file at +16 (verified Phase 4g).
        *((sbi as *const u8).add(16) as *const *mut c_void)
    };
    if file_ptr.is_null() {
        return None;
    }
    super::fs_synth::lookup_synth_file(file_ptr as usize)
        .map(|(path, _size)| path)
}

/// `new_inode(sb)` — allocate a fresh inode without going through
/// the inode hash.  Used by z_erofs_init_super for an internal
/// "managed" inode that caches decompressed pages.  Just kmalloc +
/// zero + i_sb + i_mapping.
#[unsafe(no_mangle)]
pub extern "C" fn new_inode(sb: *mut c_void) -> *mut c_void {
    let inode = super::alloc::kmalloc(fl::INODE_SIZE, 0);
    if inode.is_null() {
        log::warn!("kabi: new_inode: kmalloc failed");
        return core::ptr::null_mut();
    }
    unsafe { core::ptr::write_bytes(inode as *mut u8, 0, fl::INODE_SIZE); }
    let mapping = synth_mapping_from_sb(sb);
    unsafe {
        *(inode.cast::<u8>().add(fl::INODE_I_SB_OFF) as *mut *mut c_void) = sb;
        *(inode.cast::<u8>().add(fl::INODE_I_MAPPING_OFF)
            as *mut *mut c_void) = mapping;
        *(inode.cast::<u8>().add(fl::INODE_I_BLKBITS_OFF) as *mut u8) = 12;
    }
    log::info!("kabi: new_inode: inode={:p} sb={:p} mapping={:p}",
               inode, sb, mapping);
    inode
}

#[unsafe(no_mangle)]
pub extern "C" fn iput(_inode: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn clear_inode(_inode: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn unlock_new_inode(inode: *mut c_void) {
    log::info!("kabi-trace: unlock_new_inode(inode={:p})", inode);
    // Clear I_NEW bit (bit 0 of i_state at +144).  ext4_iget calls
    // this once it has populated the on-disk fields.
    if !inode.is_null() {
        unsafe {
            let p = inode.cast::<u8>().add(144) as *mut u32;
            *p &= !1u32;
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn init_special_inode(_inode: *mut c_void, _mode: u32,
                                     _rdev: u32) {}

#[unsafe(no_mangle)]
pub extern "C" fn inode_init_once(_inode: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn inode_nohighmem(_inode: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn inode_set_ctime_to_ts(_inode: *mut c_void,
                                        _ts: *const c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn set_nlink(inode: *mut c_void, nlink: u32) {
    log::info!("kabi-trace: set_nlink(inode={:p}, nlink={})", inode, nlink);
    // Write inode->i_nlink (offset +72; verified Phase 5).
    if !inode.is_null() && nlink > 0 {
        unsafe {
            *(inode.cast::<u8>().add(72) as *mut u32) = nlink;
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kill_anon_super(_sb: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn kill_block_super(_sb: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn get_tree_bdev_flags(_fc: *mut c_void,
                                      _fill_super: *mut c_void,
                                      _flags: u32) -> c_int {
    // Return -ENOTBLK to push erofs into its file-backed-image
    // fallback path (CONFIG_EROFS_FS_BACKED_BY_FILE), which uses
    // filp_open(fc->source) instead of opening a block device.
    // Phase 3e will replace this with a real bdev impl.
    log::warn!("kabi: get_tree_bdev_flags (stub) — returning -ENOTBLK");
    -15
}

/// `get_tree_nodev(fc, fill_super)` — Linux's "no device" mount helper.
/// Allocates an anonymous super_block, calls `fill_super(sb, fc)`, and
/// sets `fc->root` to the result.
///
/// K34 Day 2 minimum impl:
///   1. Allocate a zero-filled super_block buffer (~4KB).
///   2. Set sb->s_fs_info from fc->s_fs_info (erofs already populated).
///   3. Set sb->s_blocksize = 4096, sb->s_blocksize_bits = 12.
///   4. Call fill_super(sb, fc).
///   5. fc->root is supposed to be set by fill_super.
///   6. Return the result.
#[unsafe(no_mangle)]
pub extern "C" fn get_tree_nodev(fc: *mut c_void,
                                 fill_super: *mut c_void) -> c_int {
    super::fs_synth::get_tree_nodev_synth(fc, fill_super)
}

#[unsafe(no_mangle)]
pub extern "C" fn lockref_get_not_zero(_lr: *mut c_void) -> c_int { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn lockref_mark_dead(_lr: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn lockref_put_or_lock(_lr: *mut c_void) -> c_int { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn vfs_setpos(_file: *mut c_void, _offset: i64,
                             _maxsize: i64) -> i64 { _offset }

#[unsafe(no_mangle)]
pub extern "C" fn vfs_iocb_iter_read(_file: *mut c_void, _iocb: *mut c_void,
                                     _iter: *mut c_void) -> isize { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn iov_iter_bvec(_iter: *mut c_void, _direction: u32,
                                _bvec: *const c_void, _nr_segs: u32,
                                _count: usize) {}

#[unsafe(no_mangle)]
pub extern "C" fn fput(_file: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn filp_open(filename: *const u8, _flags: c_int,
                            _mode: u32) -> *mut c_void {
    // K34 Day 1: synthesize a struct file backed by an initramfs
    // file lookup.  The call chain at boot is:
    //   erofs's get_tree → filp_open(fc->source) → us
    // Erofs uses the returned file* for:
    //   1. file_inode(file)->i_mode S_ISREG check
    //   2. file->f_mapping->a_ops->read_folio non-null check
    //   3. then calls get_tree_nodev(fc, fill_super)
    // and during fc_fill_super, calls a_ops->read_folio to read
    // disk pages.
    //
    // Returns ERR_PTR(-errno) on failure; real struct file* on
    // success.
    if filename.is_null() {
        return super::block::err_ptr(-22); // -EINVAL
    }
    super::fs_synth::filp_open_synth(filename)
}

#[unsafe(no_mangle)]
pub extern "C" fn generic_fillattr(_idmap: *mut c_void, _request_mask: u32,
                                   _inode: *mut c_void,
                                   _stat: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn generic_file_llseek(_file: *mut c_void, _offset: i64,
                                      _whence: c_int) -> i64 { _offset }

#[unsafe(no_mangle)]
pub extern "C" fn generic_file_readonly_mmap_prepare(_file: *mut c_void,
                                                     _vma: *mut c_void) -> c_int {
    -22
}

#[unsafe(no_mangle)]
pub extern "C" fn generic_read_dir(_file: *mut c_void, _buf: *mut c_void,
                                   _siz: usize) -> isize { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn generic_setlease(_file: *mut c_void, _arg: i32,
                                   _lease: *mut c_void,
                                   _priv: *mut c_void) -> c_int { -1 }

#[unsafe(no_mangle)]
pub extern "C" fn simple_get_link(_dentry: *mut c_void, _inode: *mut c_void,
                                  _done: *mut c_void) -> *const u8 {
    core::ptr::null()
}

#[unsafe(no_mangle)]
pub extern "C" fn page_get_link(_dentry: *mut c_void, _inode: *mut c_void,
                                _done: *mut c_void) -> *const u8 {
    core::ptr::null()
}

#[unsafe(no_mangle)]
pub extern "C" fn nop_posix_acl_access(_idmap: *mut c_void,
                                       _dentry: *mut c_void,
                                       _acl: *mut c_void) -> c_int { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn nop_posix_acl_default(_idmap: *mut c_void,
                                        _dentry: *mut c_void,
                                        _acl: *mut c_void) -> c_int { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn posix_acl_from_xattr(_user_ns: *mut c_void,
                                       _value: *const c_void,
                                       _size: usize) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn noop_direct_IO(_iocb: *mut c_void,
                                 _iter: *mut c_void) -> isize { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn make_kuid(_ns: *mut c_void, _uid: u32) -> u32 { _uid }

#[unsafe(no_mangle)]
pub extern "C" fn make_kgid(_ns: *mut c_void, _gid: u32) -> u32 { _gid }

#[unsafe(no_mangle)]
pub static init_user_ns: [u8; 256] = [0; 256];

#[unsafe(no_mangle)]
pub extern "C" fn capable(_cap: c_int) -> bool { true }

ksym!(d_make_root);
ksym!(d_obtain_alias);
ksym!(d_splice_alias);
crate::ksym_static!(dotdot_name);
ksym!(iget_failed);
ksym!(iget5_locked);
ksym!(new_inode);
ksym!(iput);
ksym!(clear_inode);
ksym!(unlock_new_inode);
ksym!(init_special_inode);
ksym!(inode_init_once);
ksym!(inode_nohighmem);
ksym!(inode_set_ctime_to_ts);
ksym!(set_nlink);
ksym!(kill_anon_super);
ksym!(kill_block_super);
ksym!(get_tree_bdev_flags);
ksym!(get_tree_nodev);
ksym!(lockref_get_not_zero);
ksym!(lockref_mark_dead);
ksym!(lockref_put_or_lock);
ksym!(vfs_setpos);
ksym!(vfs_iocb_iter_read);
ksym!(iov_iter_bvec);
ksym!(fput);
ksym!(filp_open);
ksym!(generic_fillattr);
ksym!(generic_file_llseek);
ksym!(generic_file_readonly_mmap_prepare);
ksym!(generic_read_dir);
ksym!(generic_setlease);
ksym!(simple_get_link);
ksym!(page_get_link);
ksym!(nop_posix_acl_access);
ksym!(nop_posix_acl_default);
ksym!(posix_acl_from_xattr);
ksym!(noop_direct_IO);
ksym!(make_kuid);
ksym!(make_kgid);
crate::ksym_static!(init_user_ns);
ksym!(capable);

// ── iomap (return -ENOSYS / 0) ───────────────────────────────────

#[unsafe(no_mangle)]
pub static iomap_bio_read_ops: [u8; 64] = [0; 64];

#[unsafe(no_mangle)]
pub extern "C" fn iomap_bmap(_mapping: *mut c_void, _block: u64,
                             _ops: *const c_void) -> u64 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn iomap_dio_rw(_iocb: *mut c_void, _iter: *mut c_void,
                               _ops: *const c_void, _dops: *const c_void,
                               _flags: u32, _private: *mut c_void,
                               _done_before: usize) -> isize { -38 }

#[unsafe(no_mangle)]
pub extern "C" fn iomap_fiemap(_inode: *mut c_void, _fieinfo: *mut c_void,
                               _start: u64, _len: u64,
                               _ops: *const c_void) -> c_int { -38 }

#[unsafe(no_mangle)]
pub extern "C" fn iomap_invalidate_folio(_folio: *mut c_void, _offset: usize,
                                         _len: usize) {}

#[unsafe(no_mangle)]
pub extern "C" fn iomap_read_folio(_folio: *mut c_void,
                                   _ops: *const c_void) -> c_int { -38 }

#[unsafe(no_mangle)]
pub extern "C" fn iomap_readahead(_ractl: *mut c_void,
                                  _ops: *const c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn iomap_release_folio(_folio: *mut c_void,
                                      _gfp: u32) -> bool { true }

#[unsafe(no_mangle)]
pub extern "C" fn iomap_seek_data(_inode: *mut c_void, _offset: i64,
                                  _ops: *const c_void) -> i64 { _offset }

#[unsafe(no_mangle)]
pub extern "C" fn iomap_seek_hole(_inode: *mut c_void, _offset: i64,
                                  _ops: *const c_void) -> i64 { _offset }

crate::ksym_static!(iomap_bio_read_ops);
ksym!(iomap_bmap);
ksym!(iomap_dio_rw);
ksym!(iomap_fiemap);
ksym!(iomap_invalidate_folio);
ksym!(iomap_read_folio);
ksym!(iomap_readahead);
ksym!(iomap_release_folio);
ksym!(iomap_seek_data);
ksym!(iomap_seek_hole);

// ── DAX (we don't support) ───────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn dax_break_layout_final(_inode: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn dax_iomap_fault(_vmf: *mut c_void, _order: u32,
                                  _pfn: *mut c_void, _iomap_errp: *mut c_int,
                                  _ops: *const c_void) -> u32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn dax_iomap_rw(_iocb: *mut c_void, _iter: *mut c_void,
                               _ops: *const c_void) -> isize { -38 }

#[unsafe(no_mangle)]
pub extern "C" fn fs_dax_get_by_bdev(_bdev: *mut c_void, _start: *mut u64,
                                     _holder: *mut c_void,
                                     _hops: *const c_void) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn fs_put_dax(_dax_dev: *mut c_void, _holder: *mut c_void) {}

ksym!(dax_break_layout_final);
ksym!(dax_iomap_fault);
ksym!(dax_iomap_rw);
ksym!(fs_dax_get_by_bdev);
ksym!(fs_put_dax);

// ── fs_parser / parameters ───────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn __fs_parse(_log: *mut c_void, _desc: *const c_void,
                             _param: *mut c_void,
                             _result: *mut c_void) -> c_int { -22 }

#[unsafe(no_mangle)]
pub extern "C" fn fs_param_is_enum(_p: *mut c_void, _param: *mut c_void,
                                   _result: *mut c_void) -> c_int { -22 }

#[unsafe(no_mangle)]
pub extern "C" fn fs_param_is_string(_p: *mut c_void, _param: *mut c_void,
                                     _result: *mut c_void) -> c_int { -22 }

#[unsafe(no_mangle)]
pub extern "C" fn fs_param_is_u64(_p: *mut c_void, _param: *mut c_void,
                                  _result: *mut c_void) -> c_int { -22 }

/// Translate erofs's on-disk file_type byte (FT_*) to Linux's
/// `getdents`-style d_type (DT_*).  Linux 7.0 keeps this as a
/// `static inline` in `include/linux/fs.h` over a static array;
/// erofs's compiled `erofs_fill_dentries` (out-of-line in .ko)
/// calls this symbol.
///
/// FT_*  → DT_*:
///   0 UNKNOWN  → 0  DT_UNKNOWN
///   1 REG_FILE → 8  DT_REG
///   2 DIR      → 4  DT_DIR
///   3 CHRDEV   → 2  DT_CHR
///   4 BLKDEV   → 6  DT_BLK
///   5 FIFO     → 1  DT_FIFO
///   6 SOCK     → 12 DT_SOCK
///   7 SYMLINK  → 10 DT_LNK
#[unsafe(no_mangle)]
pub extern "C" fn fs_ftype_to_dtype(filetype: u32) -> u32 {
    match filetype {
        0 => 0,
        1 => 8,
        2 => 4,
        3 => 2,
        4 => 6,
        5 => 1,
        6 => 12,
        7 => 10,
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub static fs_kobj: [u8; 8] = [0; 8];

#[unsafe(no_mangle)]
pub static fs_bio_set: [u8; 64] = [0; 64];

ksym!(__fs_parse);
ksym!(fs_param_is_enum);
ksym!(fs_param_is_string);
ksym!(fs_param_is_u64);
ksym!(fs_ftype_to_dtype);
crate::ksym_static!(fs_kobj);
crate::ksym_static!(fs_bio_set);

// ── kobject / kset ───────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn kobject_init_and_add(_kobj: *mut c_void,
                                       _ktype: *mut c_void,
                                       _parent: *mut c_void,
                                       _fmt: *const u8) -> c_int {
    log::warn!("kabi-trace: kobject_init_and_add (stub returns 0)");
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn kset_register(_kset: *mut c_void) -> c_int { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn kset_unregister(_kset: *mut c_void) {}

ksym!(kobject_init_and_add);
ksym!(kset_register);
ksym!(kset_unregister);

// ── IDR (linear-scan ID-to-pointer mapping) ──────────────────────
// fs uses idr for inode tables; v1 stub returns no-op which is
// correct as long as the fs only consults idr for "is this id
// allocated?" checks (returns "not found").

#[unsafe(no_mangle)]
pub extern "C" fn idr_alloc(_idr: *mut c_void, _ptr: *mut c_void,
                            _start: c_int, _end: c_int,
                            _gfp: u32) -> c_int { -12 }

#[unsafe(no_mangle)]
pub extern "C" fn idr_destroy(_idr: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn idr_find(_idr: *mut c_void, _id: c_int) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn idr_for_each(_idr: *mut c_void, _fn: *const c_void,
                               _data: *mut c_void) -> c_int { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn idr_get_next(_idr: *mut c_void,
                               _nextid: *mut c_int) -> *mut c_void {
    core::ptr::null_mut()
}

ksym!(idr_alloc);
ksym!(idr_destroy);
ksym!(idr_find);
ksym!(idr_for_each);
ksym!(idr_get_next);

// ── xarray (radix-tree like store) ──────────────────────────────
// fs uses xarray for the page-cache mapping.  v1 stub treats
// every store as "empty" — find returns null.  add_to_page_cache
// in filemap.rs returns -EINVAL so the fs falls back to
// per-inode mapping or fails gracefully.

#[unsafe(no_mangle)]
pub extern "C" fn xa_load(_xa: *mut c_void, _index: u64) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn xa_find(_xa: *mut c_void, _index: *mut u64,
                          _max: u64, _filter: u32) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn xa_find_after(_xa: *mut c_void, _index: *mut u64,
                                _max: u64, _filter: u32) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn __xa_cmpxchg(_xa: *mut c_void, _index: u64,
                               _old: *mut c_void, _entry: *mut c_void,
                               _gfp: u32) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn __xa_erase(_xa: *mut c_void, _index: u64) -> *mut c_void {
    core::ptr::null_mut()
}

ksym!(xa_load);
ksym!(xa_find);
ksym!(xa_find_after);
ksym!(__xa_cmpxchg);
ksym!(__xa_erase);

// ── crypto / compression (return -ENOSYS so fs disables that
// compression path; erofs-uncompressed read still works) ────────

/// CRC32C (Castagnoli, polynomial `0x1EDC6F41`).
///
/// Used by erofs to validate the on-disk superblock when the
/// `EROFS_FEATURE_COMPAT_SB_CHKSUM` bit is set.  The poly is the
/// reflected/reversed form `0x82F63B78`.
///
/// Bitwise loop, no lookup table — superblock validation runs once
/// per mount, not in a hot path.  When something hot needs this,
/// pull in the Slicing-by-8 table or PMULL hw acceleration.
#[unsafe(no_mangle)]
pub extern "C" fn crc32c(crc: u32, data: *const c_void, len: usize) -> u32 {
    if data.is_null() || len == 0 {
        return crc;
    }
    let bytes = unsafe { core::slice::from_raw_parts(data as *const u8, len) };
    let mut c = crc;
    for &b in bytes {
        c ^= b as u32;
        for _ in 0..8 {
            let mask = (c & 1).wrapping_neg();
            c = (c >> 1) ^ (0x82F6_3B78 & mask);
        }
    }
    c
}

#[unsafe(no_mangle)]
pub extern "C" fn crypto_alloc_acomp(_alg_name: *const u8, _type_: u32,
                                     _mask: u32) -> *mut c_void {
    log::warn!("kabi-trace: crypto_alloc_acomp (stub) — null");
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn crypto_acomp_decompress(_req: *mut c_void) -> c_int { -38 }

#[unsafe(no_mangle)]
pub extern "C" fn crypto_destroy_tfm(_mem: *mut c_void, _tfm: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn crypto_req_done(_data: *mut c_void, _err: c_int) {}

#[unsafe(no_mangle)]
pub extern "C" fn xxh32(_input: *const c_void, _length: usize, _seed: u32) -> u32 {
    0
}

// LZ4 decompress family
#[unsafe(no_mangle)]
pub extern "C" fn LZ4_decompress_safe(_source: *const u8, _dest: *mut u8,
                                      _csize: c_int, _dsize: c_int) -> c_int {
    -1
}
#[unsafe(no_mangle)]
pub extern "C" fn LZ4_decompress_safe_partial(_source: *const u8, _dest: *mut u8,
                                              _csize: c_int, _tsize: c_int,
                                              _dsize: c_int) -> c_int {
    -1
}

// xz microlzma
#[unsafe(no_mangle)]
pub extern "C" fn xz_dec_microlzma_alloc(_mode: c_int, _dict_size: u32) -> *mut c_void {
    core::ptr::null_mut()
}
#[unsafe(no_mangle)]
pub extern "C" fn xz_dec_microlzma_end(_s: *mut c_void) {}
#[unsafe(no_mangle)]
pub extern "C" fn xz_dec_microlzma_reset(_s: *mut c_void, _comp_size: u32,
                                         _uncomp_size: u32, _force: c_int) {}
#[unsafe(no_mangle)]
pub extern "C" fn xz_dec_microlzma_run(_s: *mut c_void,
                                       _b: *mut c_void) -> c_int { -1 }

// zlib inflate
#[unsafe(no_mangle)]
pub extern "C" fn zlib_inflate(_strm: *mut c_void, _flush: c_int) -> c_int { -2 }
#[unsafe(no_mangle)]
pub extern "C" fn zlib_inflate_workspacesize() -> c_int { 0 }
#[unsafe(no_mangle)]
pub extern "C" fn zlib_inflateEnd(_strm: *mut c_void) -> c_int { 0 }
#[unsafe(no_mangle)]
pub extern "C" fn zlib_inflateInit2(_strm: *mut c_void,
                                    _windowbits: c_int) -> c_int { -2 }

// zstd
#[unsafe(no_mangle)]
pub extern "C" fn zstd_decompress_stream(_dctx: *mut c_void,
                                         _output: *mut c_void,
                                         _input: *mut c_void) -> usize { 0 }
#[unsafe(no_mangle)]
pub extern "C" fn zstd_dstream_workspace_bound(_max_window_size: usize) -> usize { 0 }
#[unsafe(no_mangle)]
pub extern "C" fn zstd_get_error_name(_code: usize) -> *const u8 {
    b"kabi-stub-zstd-error\0".as_ptr()
}
#[unsafe(no_mangle)]
pub extern "C" fn zstd_init_dstream(_max_window_size: usize, _workspace: *mut c_void,
                                    _workspace_size: usize) -> *mut c_void {
    log::warn!("kabi-trace: zstd_init_dstream (stub) — null");
    core::ptr::null_mut()
}
#[unsafe(no_mangle)]
pub extern "C" fn zstd_is_error(_code: usize) -> bool { true }

ksym!(crc32c);
ksym!(crypto_alloc_acomp);
ksym!(crypto_acomp_decompress);
ksym!(crypto_destroy_tfm);
ksym!(crypto_req_done);
ksym!(xxh32);
ksym!(LZ4_decompress_safe);
ksym!(LZ4_decompress_safe_partial);
ksym!(xz_dec_microlzma_alloc);
ksym!(xz_dec_microlzma_end);
ksym!(xz_dec_microlzma_reset);
ksym!(xz_dec_microlzma_run);
ksym!(zlib_inflate);
ksym!(zlib_inflate_workspacesize);
ksym!(zlib_inflateEnd);
ksym!(zlib_inflateInit2);
ksym!(zstd_decompress_stream);
ksym!(zstd_dstream_workspace_bound);
ksym!(zstd_get_error_name);
ksym!(zstd_init_dstream);
ksym!(zstd_is_error);

// ── Workqueue / kthread / shrinker ───────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn alloc_workqueue_noprof(_fmt: *const u8, _flags: u32,
                                         _max_active: c_int) -> *mut c_void {
    log::warn!("kabi-trace: alloc_workqueue_noprof (stub) — fake handle");
    // Return non-null so callers' "if (!wq)" check passes.  v1
    // doesn't actually run the queue; queue_work_on returns false.
    super::alloc::kmalloc(64, 0)
}
#[unsafe(no_mangle)]
pub extern "C" fn destroy_workqueue(_wq: *mut c_void) {}
#[unsafe(no_mangle)]
pub extern "C" fn queue_work_on(_cpu: c_int, _wq: *mut c_void,
                                _work: *mut c_void) -> bool { false }
#[unsafe(no_mangle)]
pub extern "C" fn kthread_create_worker_on_cpu(_cpu: c_int, _flags: u32,
                                               _namefmt: *const u8) -> *mut c_void {
    core::ptr::null_mut()
}
#[unsafe(no_mangle)]
pub extern "C" fn kthread_destroy_worker(_w: *mut c_void) {}
#[unsafe(no_mangle)]
pub extern "C" fn kthread_queue_work(_w: *mut c_void,
                                     _kw: *mut c_void) -> bool { false }
// shrinker_alloc returns a `struct shrinker *` that the caller
// writes fields into (count_objects, scan_objects, seeks, …) and
// then passes to shrinker_register.  We don't run shrinkers
// (memory pressure callbacks); a 256-byte heap buffer is plenty
// of room for the caller's writes and harmless because
// shrinker_register never actually invokes the callbacks.
#[unsafe(no_mangle)]
pub extern "C" fn shrinker_alloc(_flags: u32, _fmt: *const u8) -> *mut c_void {
    log::warn!("kabi: shrinker_alloc (stub) — 256-byte fake handle");
    super::alloc::kmalloc(256, 0)
}
#[unsafe(no_mangle)]
pub extern "C" fn shrinker_free(shrinker: *mut c_void) {
    if !shrinker.is_null() {
        super::alloc::kfree(shrinker);
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn shrinker_register(_shrinker: *mut c_void) -> c_int { 0 }

ksym!(alloc_workqueue_noprof);
ksym!(destroy_workqueue);
ksym!(queue_work_on);
ksym!(kthread_create_worker_on_cpu);
ksym!(kthread_destroy_worker);
ksym!(kthread_queue_work);
ksym!(shrinker_alloc);
ksym!(shrinker_free);
ksym!(shrinker_register);

// ── Sync primitives — sleep/wake, RCU, locks ─────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn __init_rwsem(_sem: *mut c_void, _name: *const u8,
                               _key: *mut c_void) {}
#[unsafe(no_mangle)]
pub extern "C" fn __init_swait_queue_head(_q: *mut c_void, _name: *const u8,
                                          _key: *mut c_void) {}
#[unsafe(no_mangle)]
pub extern "C" fn down_read(_sem: *mut c_void) {}
#[unsafe(no_mangle)]
pub extern "C" fn down_write(_sem: *mut c_void) {}
#[unsafe(no_mangle)]
pub extern "C" fn up_read(_sem: *mut c_void) {}
#[unsafe(no_mangle)]
pub extern "C" fn up_write(_sem: *mut c_void) {}
#[unsafe(no_mangle)]
pub extern "C" fn mutex_init_generic(_m: *mut c_void) {}
#[unsafe(no_mangle)]
pub extern "C" fn mutex_trylock(_m: *mut c_void) -> c_int { 1 }
#[unsafe(no_mangle)]
pub extern "C" fn _raw_spin_trylock(_lock: *mut c_void) -> c_int { 1 }
#[unsafe(no_mangle)]
pub extern "C" fn __wake_up(_q: *mut c_void, _mode: u32, _nr: c_int,
                            _key: *mut c_void) {}
#[unsafe(no_mangle)]
pub extern "C" fn wake_up_bit(_word: *const c_void, _bit: c_int) {}
#[unsafe(no_mangle)]
pub extern "C" fn wake_up_process(_t: *mut c_void) -> c_int { 1 }
#[unsafe(no_mangle)]
pub extern "C" fn prepare_to_wait_event(_q: *mut c_void, _wait: *mut c_void,
                                        _state: c_int) -> c_int { 0 }
#[unsafe(no_mangle)]
pub extern "C" fn finish_wait(_q: *mut c_void, _wait: *mut c_void) {}
#[unsafe(no_mangle)]
pub extern "C" fn init_wait_entry(_wait: *mut c_void, _flags: c_int) {}
#[unsafe(no_mangle)]
pub extern "C" fn out_of_line_wait_on_bit_lock(_word: *const c_void,
                                                _bit: c_int,
                                                _action: *const c_void,
                                                _mode: u32) -> c_int { 0 }
#[unsafe(no_mangle)]
pub extern "C" fn bit_wait(_word: *mut c_void) -> c_int { 0 }
#[unsafe(no_mangle)]
pub extern "C" fn wait_for_completion_io(_x: *mut c_void) {}
#[unsafe(no_mangle)]
pub extern "C" fn __rcu_read_lock() {}
#[unsafe(no_mangle)]
pub extern "C" fn __rcu_read_unlock() {}
#[unsafe(no_mangle)]
pub extern "C" fn call_rcu(_head: *mut c_void, _func: *const c_void) {}
#[unsafe(no_mangle)]
pub extern "C" fn synchronize_rcu() {}
#[unsafe(no_mangle)]
pub extern "C" fn rcu_barrier() {}
#[unsafe(no_mangle)]
pub extern "C" fn dynamic_preempt_schedule_notrace() {}

ksym!(__init_rwsem);
ksym!(__init_swait_queue_head);
ksym!(down_read);
ksym!(down_write);
ksym!(up_read);
ksym!(up_write);
ksym!(mutex_init_generic);
ksym!(mutex_trylock);
ksym!(_raw_spin_trylock);
ksym!(__wake_up);
ksym!(wake_up_bit);
ksym!(wake_up_process);
ksym!(prepare_to_wait_event);
ksym!(finish_wait);
ksym!(init_wait_entry);
ksym!(out_of_line_wait_on_bit_lock);
ksym!(bit_wait);
ksym!(wait_for_completion_io);
ksym!(__rcu_read_lock);
ksym!(__rcu_read_unlock);
ksym!(call_rcu);
ksym!(synchronize_rcu);
ksym!(rcu_barrier);
ksym!(dynamic_preempt_schedule_notrace);

// ── Page allocator wrappers (Linux-named) ───────────────────────
// Real impls — delegate to platform::page_allocator + kabi::alloc.
// erofs's init does several `__alloc_pages_node()`-shaped calls
// for its inode-cache slab pages and shrinker book-keeping.  Null
// returns make init bail with -ENOMEM.

#[unsafe(no_mangle)]
pub extern "C" fn __free_pages(page: *mut c_void, _order: u32) {
    // Linux's `struct page *` is the page-frame descriptor.  In our
    // model `kmalloc` returns a kernel direct-map VA, and we used
    // that VA as the "page" pointer in alloc_pages_noprof.  Free
    // through the kabi::alloc kfree path which understands both
    // small heap allocations and full-page kmallocs.
    super::alloc::kfree(page);
}

#[unsafe(no_mangle)]
pub extern "C" fn alloc_pages_noprof(gfp: u32, order: u32) -> *mut c_void {
    // 2^order pages.  K33 v1: implement up to order=4 (64 KiB)
    // via kabi::alloc::kmalloc with a page-aligned size.  Anything
    // beyond that returns null; erofs's hot path is order-0/1.
    let size = (1usize << order) * kevlar_platform::arch::PAGE_SIZE;
    if size > 64 * 1024 {
        log::warn!("kabi: alloc_pages_noprof order={} too large", order);
        return core::ptr::null_mut();
    }
    super::alloc::kmalloc(size, gfp)
}

#[unsafe(no_mangle)]
pub extern "C" fn alloc_pages_bulk_noprof(gfp: u32, nr_pages: usize,
                                          page_array: *mut *mut c_void) -> usize {
    if page_array.is_null() {
        return 0;
    }
    let mut filled = 0usize;
    for i in 0..nr_pages {
        let p = super::alloc::kmalloc(kevlar_platform::arch::PAGE_SIZE, gfp);
        if p.is_null() {
            break;
        }
        unsafe { *page_array.add(i) = p };
        filled += 1;
    }
    filled
}

#[unsafe(no_mangle)]
pub extern "C" fn vmalloc_noprof(size: usize) -> *mut c_void {
    super::alloc::vmalloc(size)
}

#[unsafe(no_mangle)]
pub extern "C" fn vmap(_pages: *mut *mut c_void, _count: u32, _flags: u32,
                       _prot: u32) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn vunmap(_addr: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn vm_map_ram(_pages: *mut *mut c_void, _count: u32,
                             _node: c_int) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn vm_unmap_ram(_mem: *mut c_void, _count: u32) {}

#[unsafe(no_mangle)]
pub extern "C" fn vm_unmap_aliases() {}

#[unsafe(no_mangle)]
pub extern "C" fn thp_get_unmapped_area(_filp: *mut c_void, _addr: u64,
                                        _len: usize, _pgoff: u64,
                                        _flags: u32) -> u64 { 0 }

ksym!(__free_pages);
ksym!(alloc_pages_noprof);
ksym!(alloc_pages_bulk_noprof);
ksym!(vmalloc_noprof);
ksym!(vmap);
ksym!(vunmap);
ksym!(vm_map_ram);
ksym!(vm_unmap_ram);
ksym!(vm_unmap_aliases);
ksym!(thp_get_unmapped_area);

// ── slab kmemcache wrappers ─────────────────────────────────────
// Wired to the existing kmem_cache_alloc_noprof in kabi::slab so
// erofs's inode-cache allocations succeed.  The "_lru" variant
// just adds an LRU-list hint for memory-pressure-aware caches;
// we ignore it (no LRU tracking yet).

#[unsafe(no_mangle)]
pub extern "C" fn kmem_cache_alloc_lru_noprof(cache: *mut c_void,
                                              _lru: *mut c_void,
                                              gfp: u32) -> *mut c_void {
    super::slab::kmem_cache_alloc_noprof(cache, gfp)
}

#[unsafe(no_mangle)]
pub extern "C" fn krealloc_node_align_noprof(p: *mut c_void, new_size: usize,
                                             _align: usize, gfp: u32,
                                             _node: c_int) -> *mut c_void {
    // Best-effort: free the old + alloc the new.  We don't have a
    // realloc primitive in kabi::alloc; size is opaque on free, so
    // we can't actually preserve old contents.  v1: kmalloc fresh.
    // erofs uses this rarely; if a real caller bites, write a real
    // realloc in kabi::alloc.
    if !p.is_null() {
        super::alloc::kfree(p);
    }
    super::alloc::kmalloc(new_size, gfp)
}

#[unsafe(no_mangle)]
pub extern "C" fn kfree_sensitive(p: *mut c_void) {
    super::alloc::kfree(p);
}

#[unsafe(no_mangle)]
pub extern "C" fn kmemdup_nul(s: *const u8, len: usize, gfp: u32) -> *mut u8 {
    if s.is_null() {
        return core::ptr::null_mut();
    }
    let buf = super::alloc::kmalloc(len + 1, gfp) as *mut u8;
    if buf.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        core::ptr::copy_nonoverlapping(s, buf, len);
        *buf.add(len) = 0;
    }
    buf
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kstrdup(s: *const u8, gfp: u32) -> *mut u8 {
    if s.is_null() {
        return core::ptr::null_mut();
    }
    // Bounded strlen; fs callers pass NUL-terminated names ≤ 256.
    let len = unsafe { super::mem::strnlen(s, 4096) };
    kmemdup_nul(s, len, gfp)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kstrndup(s: *const u8, max: usize, gfp: u32) -> *mut u8 {
    if s.is_null() {
        return core::ptr::null_mut();
    }
    let len = unsafe { super::mem::strnlen(s, max) };
    kmemdup_nul(s, len, gfp)
}

ksym!(kmem_cache_alloc_lru_noprof);
ksym!(krealloc_node_align_noprof);
ksym!(kfree_sensitive);
ksym!(kmemdup_nul);
ksym!(kstrdup);
ksym!(kstrndup);

// ── Misc print/seq/sysfs ────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn vsnprintf(buf: *mut u8, size: usize, _fmt: *const u8,
                            _args: *mut c_void) -> c_int {
    // v1: just NUL-terminate.  TODO: wire to platform printf.
    if !buf.is_null() && size > 0 {
        unsafe { *buf = 0 };
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn scnprintf(buf: *mut u8, size: usize,
                            _fmt: *const u8) -> c_int {
    // v1: just NUL-terminate the buffer.  Real Linux is variadic
    // and does the formatting; module callers usually use this for
    // diagnostic strings that don't reach userspace, so a no-op is
    // safe.  TODO: hook to platform printf.
    if !buf.is_null() && size > 0 {
        unsafe { *buf = 0 };
    }
    0
}

ksym!(vsnprintf);

#[unsafe(no_mangle)]
pub extern "C" fn sprintf(_buf: *mut u8, _fmt: *const u8) -> c_int { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn seq_printf(_seq: *mut c_void, _fmt: *const u8) {}

#[unsafe(no_mangle)]
pub extern "C" fn seq_write(_seq: *mut c_void, _data: *const c_void,
                            _len: usize) -> c_int { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn sysfs_emit(_buf: *mut u8, _fmt: *const u8) -> c_int { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn logfc(_log: *mut c_void, _prefix: *const u8,
                        _level: u32, _fmt: *const u8) {}

ksym!(scnprintf);
ksym!(sprintf);
ksym!(seq_printf);
ksym!(seq_write);
ksym!(sysfs_emit);
ksym!(logfc);

// ── CPU mask / hotplug / arch globals ───────────────────────────

#[unsafe(no_mangle)]
pub static __cpu_online_mask: [u8; 64] = {
    let mut m = [0u8; 64];
    m[0] = 1; m
};

#[unsafe(no_mangle)]
pub static __cpu_possible_mask: [u8; 64] = {
    let mut m = [0u8; 64];
    m[0] = 1; m
};

#[unsafe(no_mangle)]
pub static __num_possible_cpus: u32 = 1;

#[unsafe(no_mangle)]
pub static nr_cpu_ids: u32 = 1;

#[unsafe(no_mangle)]
pub static numa_node: u32 = 0;

#[unsafe(no_mangle)]
pub static arm64_use_ng_mappings: bool = false;

#[unsafe(no_mangle)]
pub extern "C" fn __cpuhp_setup_state(_state: c_int, _name: *const u8,
                                      _invoke: c_int, _startup: *const c_void,
                                      _teardown: *const c_void,
                                      _multi_instance: c_int) -> c_int { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn __cpuhp_remove_state(_state: c_int, _invoke: c_int) {}

#[unsafe(no_mangle)]
pub extern "C" fn migrate_disable() {}

#[unsafe(no_mangle)]
pub extern "C" fn migrate_enable() {}

#[unsafe(no_mangle)]
pub extern "C" fn _find_next_bit(_addr: *const u64, _size: u64,
                                 _start: u64) -> u64 { _size }

crate::ksym_static!(__cpu_online_mask);
crate::ksym_static!(__cpu_possible_mask);
crate::ksym_static!(__num_possible_cpus);
crate::ksym_static!(nr_cpu_ids);
crate::ksym_static!(numa_node);
crate::ksym_static!(arm64_use_ng_mappings);
ksym!(__cpuhp_setup_state);
ksym!(__cpuhp_remove_state);
ksym!(migrate_disable);
ksym!(migrate_enable);
ksym!(_find_next_bit);

// ── psi memstall (pressure-stall info) — no-op ─────────────────

#[unsafe(no_mangle)]
pub extern "C" fn psi_memstall_enter(_flags: *mut u64) {}

#[unsafe(no_mangle)]
pub extern "C" fn psi_memstall_leave(_flags: *mut u64) {}

ksym!(psi_memstall_enter);
ksym!(psi_memstall_leave);

// ── usercopy (Linux-named copy_to/from_user trampolines) ────────
// Existing kabi/usercopy.rs exports `copy_to_user` / `copy_from_user`
// under those names.  arm64 also references the assembly entry
// points `__arch_copy_to_user` / `__arch_clear_user` directly.
// Wire them to the same impl.

#[unsafe(no_mangle)]
pub extern "C" fn __arch_copy_to_user(_dst: *mut c_void, _src: *const c_void,
                                      _n: usize) -> usize {
    // TODO: wire to platform copy_to_user.  For now, no-op:
    // returns "0 bytes uncopied" (success).
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn __arch_clear_user(_dst: *mut c_void, _n: usize) -> usize {
    0
}

ksym!(__arch_copy_to_user);
ksym!(__arch_clear_user);

// ── scatterlist ─────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn sg_alloc_table_from_pages_segment(
    _sgt: *mut c_void, _pages: *mut *mut c_void, _n_pages: u32,
    _offset: u32, _size: usize, _max_segment: u32, _prv: *mut c_void,
    _left_pages: u32, _gfp: u32) -> c_int { -38 }

#[unsafe(no_mangle)]
pub extern "C" fn sg_free_table(_sgt: *mut c_void) {}

ksym!(sg_alloc_table_from_pages_segment);
ksym!(sg_free_table);

// ── Misc globals ─────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub static uuid_null: [u8; 16] = [0; 16];

#[unsafe(no_mangle)]
pub static param_ops_uint: [u8; 64] = [0; 64];

crate::ksym_static!(uuid_null);
crate::ksym_static!(param_ops_uint);
