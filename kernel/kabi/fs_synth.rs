// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Synthesised Linux structs handed back to a kABI-loaded fs `.ko`
//! via `filp_open`.  K34 Day 1.
//!
//! Erofs's mount path opens a backing file with `filp_open(path)`,
//! then reads two fields off the result:
//!
//!   1. `S_ISREG(file_inode(file)->i_mode)` — must be true.
//!   2. `file->f_mapping->a_ops->read_folio` — must be non-null.
//!
//! ...and later calls `read_folio(file, folio)` to read disk
//! pages.  Day 1 covers (1) and (2) — handing back a struct that
//! satisfies the checks; Day 2 implements `read_folio`.
//!
//! Side-table maps the synthesised `struct file *` address back to
//! the originating path so `read_folio` can look up the initramfs
//! file when it's called later.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};

use kevlar_platform::spinlock::SpinLock;

use super::struct_layouts as fl;

/// Pointer to the kABI address_space_operations table, allocated
/// on the heap at boot via `init_synth()`.  Stored as an atomic
/// so reads from filp_open_synth and init_synth see the same
/// value (a `pub static [usize; N]` was getting const-evaluated
/// to multiple addresses, defeating the indirect-call setup).
static KABI_AOPS_PTR: AtomicUsize = AtomicUsize::new(0);

/// Day 2 gate: erofs's fill_super dispatch crashes on inline
/// `kmap_local_page` expansions that target Linux-PAGE_OFFSET VAs
/// we don't map.  Until VA aliasing is implemented (K35+), keep
/// the call gated off.  Set via cmdline `kabi-fill-super=1`.
pub static mut ALLOW_FILL_SUPER: bool = false;

/// Side channel between get_tree_nodev_synth and the
/// kabi_mount_filesystem caller in fs_adapter.rs.  After fill_super
/// succeeds we stash (sb, root_dentry) here so the adapter can wrap
/// them in `KabiFileSystem` without walking dentry->d_sb (whose
/// offset is currently a guess).  Single-mount v1; revisit when we
/// need concurrent mounts.
pub static LAST_MOUNT_STATE: SpinLock<Option<(usize, usize)>> =
    SpinLock::new(None);

/// Returns the address of the kABI aops table.  Panics if
/// init_synth() hasn't been called.
fn kabi_aops_addr() -> usize {
    let p = KABI_AOPS_PTR.load(Ordering::Acquire);
    debug_assert!(p != 0, "fs_synth: kabi_aops not initialised");
    p
}

/// Per-file backing record.  Lives as long as the synthesised
/// struct file does.
#[derive(Clone)]
struct SynthFile {
    /// Address of the synthesised `struct file` we handed out.
    file_ptr: usize,
    /// Address of the synthesised `struct inode`.
    inode_ptr: usize,
    /// Address of the synthesised `struct address_space`.
    mapping_ptr: usize,
    /// Source path in the initramfs.
    path: String,
    /// File size in bytes (initialised at filp_open time).
    size: u64,
}

/// Side-table: file_ptr → backing record.  Walked by read_folio
/// and by fput.  Single-mount K34 v1: a Vec is fine; Phase 4 will
/// hash on file_ptr.
static SYNTH_FILES: SpinLock<Vec<SynthFile>> = SpinLock::new(Vec::new());

/// Look up the SynthFile record for a given file pointer.
/// Returns the backing path + size.
pub fn lookup_synth_file(file_ptr: usize) -> Option<(String, u64)> {
    let table = SYNTH_FILES.lock();
    table.iter().find(|s| s.file_ptr == file_ptr)
        .map(|s| (s.path.clone(), s.size))
}

/// Look up the backing path for a file or address_space pointer.
/// Used by `read_cache_folio` when erofs passes either the file
/// or the file's f_mapping as the mapping argument.
pub fn lookup_synth_file_path_for_mapping(
    mapping: *mut c_void,
    file: *mut c_void,
) -> Option<String> {
    let mapping_addr = mapping as usize;
    let file_addr = file as usize;
    let table = SYNTH_FILES.lock();
    for s in table.iter() {
        if s.file_ptr == file_addr || s.mapping_ptr == mapping_addr {
            return Some(s.path.clone());
        }
    }
    None
}

/// Synthesise a struct file backed by the given path.  Allocates
/// the file + inode + address_space + populates fields erofs
/// reads at mount time.
pub fn filp_open_synth(path_ptr: *const u8) -> *mut c_void {
    let path = unsafe { read_cstr(path_ptr, 256) };
    log::info!("kabi: filp_open_synth({:?})", path);

    // Look up file size in initramfs to set i_size.
    let size = match initramfs_file_size(&path) {
        Some(s) => s,
        None => {
            log::warn!("kabi: filp_open_synth: {} not found in initramfs", path);
            return super::block::err_ptr(-2); // -ENOENT
        }
    };

    // Allocate the four backing structs in zero-initialised heap.
    let file_buf = super::alloc::kmalloc(fl::FILE_SIZE, 0);
    let inode_buf = super::alloc::kmalloc(fl::INODE_SIZE, 0);
    let mapping_buf = super::alloc::kmalloc(fl::AS_SIZE, 0);
    if file_buf.is_null() || inode_buf.is_null() || mapping_buf.is_null() {
        log::warn!("kabi: filp_open_synth: kmalloc failed");
        // Best-effort cleanup; on partial-alloc failure we leak the
        // ones that did succeed.  Boot-time so harmless.
        return super::block::err_ptr(-12); // -ENOMEM
    }
    unsafe {
        core::ptr::write_bytes(file_buf as *mut u8, 0, fl::FILE_SIZE);
        core::ptr::write_bytes(inode_buf as *mut u8, 0, fl::INODE_SIZE);
        core::ptr::write_bytes(mapping_buf as *mut u8, 0, fl::AS_SIZE);
    }

    // Populate struct file fields erofs reads.
    unsafe {
        let f = file_buf.cast::<u8>();
        // f_mapping at +16
        *(f.add(fl::FILE_F_MAPPING_OFF) as *mut *mut c_void) = mapping_buf;
        // f_inode at +32
        *(f.add(fl::FILE_F_INODE_OFF) as *mut *mut c_void) = inode_buf;
    }

    // Populate struct inode fields.
    unsafe {
        let i = inode_buf.cast::<u8>();
        // i_mode at +0 = S_IFREG | 0644
        *(i.add(fl::INODE_I_MODE_OFF) as *mut u16) = fl::S_IFREG | 0o644;
        // i_size at +80
        *(i.add(fl::INODE_I_SIZE_OFF) as *mut i64) = size as i64;
        // i_mapping at +48 (some erofs paths read this)
        *(i.add(fl::INODE_I_MAPPING_OFF) as *mut *mut c_void) = mapping_buf;
    }

    // Populate struct address_space fields.
    unsafe {
        let m = mapping_buf.cast::<u8>();
        // host (= inode) at +0
        *(m.add(fl::AS_HOST_OFF) as *mut *mut c_void) = inode_buf;
        // a_ops at AS_A_OPS_OFF — point at the heap-allocated kabi
        // aops table whose [read_folio] slot is filled at boot.
        *(m.add(fl::AS_A_OPS_OFF) as *mut usize) = kabi_aops_addr();
    }

    // Record in side-table for read_folio lookup.
    let record = SynthFile {
        file_ptr: file_buf as usize,
        inode_ptr: inode_buf as usize,
        mapping_ptr: mapping_buf as usize,
        path: path.clone(),
        size,
    };
    SYNTH_FILES.lock().push(record);

    log::info!(
        "kabi: filp_open_synth: file={:p} inode={:p} mapping={:p} size={} for {}",
        file_buf, inode_buf, mapping_buf, size, path,
    );

    // Verification: read back the fields erofs will read.
    unsafe {
        let f = file_buf.cast::<u8>();
        let read_f_mapping = *(f.add(fl::FILE_F_MAPPING_OFF) as *const usize);
        let read_f_inode = *(f.add(fl::FILE_F_INODE_OFF) as *const usize);
        let i = inode_buf.cast::<u8>();
        let read_i_mode = *(i.add(fl::INODE_I_MODE_OFF) as *const u16);
        let m = mapping_buf.cast::<u8>();
        let read_a_ops = *(m.add(fl::AS_A_OPS_OFF) as *const usize);
        let aops_table = read_a_ops as *const usize;
        let read_read_folio = if aops_table.is_null() { 0 }
            else { *aops_table.add(fl::AOPS_READ_FOLIO_OFF / 8) };
        log::info!(
            "kabi: filp_open_synth verify: f_mapping={:#x} f_inode={:#x} \
             i_mode={:#o} a_ops={:#x} read_folio={:#x}",
            read_f_mapping, read_f_inode, read_i_mode, read_a_ops, read_read_folio,
        );
    }

    file_buf
}

/// Read a NUL-terminated UTF-8 path from a C string pointer.
unsafe fn read_cstr(ptr: *const u8, max: usize) -> String {
    let mut buf = Vec::with_capacity(max);
    for i in 0..max {
        let c = unsafe { *ptr.add(i) };
        if c == 0 { break; }
        buf.push(c);
    }
    String::from_utf8_lossy(&buf).to_string()
}

/// Look up file size in initramfs.  Returns None if not found.
fn initramfs_file_size(path: &str) -> Option<u64> {
    use kevlar_vfs::file_system::FileSystem;
    let initramfs = crate::fs::initramfs::INITRAM_FS.clone();
    let mut current = initramfs.root_dir().ok()?;
    let mut iter = path.split('/').filter(|c| !c.is_empty()).peekable();
    while let Some(component) = iter.next() {
        let inode = current.lookup(component).ok()?;
        if iter.peek().is_some() {
            current = inode.as_dir().ok()?.clone();
        } else {
            // Last component — get file size via stat.
            let file = inode.as_file().ok()?;
            return file.stat().ok().map(|s| s.size.0 as u64);
        }
    }
    None
}

/// `get_tree_nodev`: allocate a synth super_block, call fill_super,
/// set fc->root.  Linux's helper expanded out by hand because the
/// real one allocates dentries and inodes through a chain of
/// helpers we don't have.  K34 Day 2: drive erofs's
/// fc_fill_super and observe the next failure mode.
pub fn get_tree_nodev_synth(fc: *mut c_void,
                            fill_super: *mut c_void) -> i32 {
    log::warn!(
        "kabi: get_tree_nodev_synth(fc={:p}, fill_super={:p})",
        fc, fill_super,
    );

    if fc.is_null() || fill_super.is_null() {
        return -22; // -EINVAL
    }

    // Allocate a zero-filled super_block buffer.
    let sb = super::alloc::kmalloc(fl::SB_SIZE, 0);
    if sb.is_null() {
        return -12; // -ENOMEM
    }
    unsafe { core::ptr::write_bytes(sb as *mut u8, 0, fl::SB_SIZE); }

    // Propagate fc->s_fs_info → sb->s_fs_info (Linux's
    // vfs_get_super does this; we replace it).  Without this,
    // EROFS_SB(sb) returns null and erofs's first-line read of
    // sbi->ino_oversion in erofs_squash_ino faults.
    let fc_s_fs_info = unsafe {
        *(fc.cast::<u8>().add(fl::FC_S_FS_INFO_OFF) as *const *mut c_void)
    };

    // Set basic super_block fields with offsets verified against
    // erofs.ko disasm at fc_fill_super offset 0x49c0.
    unsafe {
        let s = sb.cast::<u8>();
        *(s.add(fl::SB_S_BLOCKSIZE_BITS_OFF) as *mut u8) = 12;
        *(s.add(fl::SB_S_BLOCKSIZE_OFF) as *mut u64) = 4096;
        *(s.add(fl::SB_S_MAXBYTES_OFF) as *mut i64) = i64::MAX;
        *(s.add(fl::SB_S_FS_INFO_OFF) as *mut *mut c_void) = fc_s_fs_info;
    }
    log::info!(
        "kabi: get_tree_nodev_synth: propagating fc->s_fs_info={:p} to sb->s_fs_info",
        fc_s_fs_info,
    );

    // K34 Day 2 finding: dispatching fill_super takes erofs deep
    // into its on-disk superblock read path.  At PC offset 0x49d0
    // inside erofs_iget5_set the .ko's compiled code does an inline
    // `kmap_local_page` expansion that produces VAs in Linux's
    // PAGE_OFFSET region.  Linux 7.0 arm64 builds with VA_BITS=52
    // → PAGE_OFFSET = 0xfff0_0000_0000_0000; Kevlar's KERNEL_BASE
    // = 0xffff_0000_0000_0000.  The two direct maps don't overlap,
    // so erofs's pointer arithmetic produces unmapped VAs:
    //
    //   panicked at platform/arm64/interrupt.rs:136:17:
    //   kernel page fault: pc=0xffff00007cdc49d0
    //                      far=0xffff_8010_0000_0400
    //                      esr=0x96000004
    //
    // Fixing this requires K35+ work — either:
    //   A. Realign Kevlar's kernel VA layout to Linux 7.0's
    //      PAGE_OFFSET (multi-week kernel mm work).
    //   B. Set up an alias mapping that places our paddrs at
    //      Linux-compat VAs (arm64 page-table extension).
    //   C. Build a custom erofs.ko with kABI hooks that bypass
    //      the inline VA math (defeats "drop-in Linux replacement").
    //
    // Gate the dispatch behind `kabi-fill-super=1` so default boot
    // + kabi-load-erofs=1 stay clean.  When the gate is off,
    // return -ENOSYS — the kABI control flow ran end-to-end but
    // the data flow needs Linux-compat VA mappings to proceed.
    let allow_fill = unsafe { ALLOW_FILL_SUPER };
    if !allow_fill {
        log::warn!(
            "kabi: get_tree_nodev_synth: fill_super gated off (need \
             `kabi-fill-super=1` cmdline + Linux-compat VA aliasing). \
             erofs control flow proven; data flow blocked at VA layout.",
        );
        super::alloc::kfree(sb);
        return -38; // -ENOSYS
    }

    log::info!("kabi: get_tree_nodev_synth: sb={:p}, calling fill_super", sb);

    // SCS hand-off — see kabi::loader::call_with_scs_2 for rationale.
    // fill_super(sb, fc) → 2 ptr args.
    let rc = super::loader::call_with_scs_2(
        fill_super as *const (), sb as usize, fc as usize,
    ) as i32;
    log::info!("kabi: fill_super dispatch returned rc={}", rc);

    if rc < 0 {
        log::warn!(
            "kabi: get_tree_nodev_synth: fill_super returned {} — bailing",
            rc,
        );
        log::info!("kabi: about to kfree sb={:p}", sb);
        super::alloc::kfree(sb);
        log::info!("kabi: kfree done; returning rc={}", rc);
        return rc;
    }

    log::info!(
        "kabi: get_tree_nodev_synth: fill_super returned 0; \
         setting fc->root from sb->s_root",
    );

    // Read sb->s_root (set by fill_super) and write to fc->root.
    let s_root = unsafe {
        *(sb.cast::<u8>().add(fl::SB_S_ROOT_OFF) as *const *mut c_void)
    };
    if s_root.is_null() {
        log::warn!("kabi: get_tree_nodev_synth: sb->s_root is null after fill_super");
        return -22;
    }
    unsafe {
        *(fc.cast::<u8>().add(fl::FC_ROOT_OFF) as *mut *mut c_void) = s_root;
    }

    log::info!(
        "kabi: get_tree_nodev_synth: fc->root = {:p} — mount succeeded",
        s_root,
    );
    // Side-channel for kabi_mount_filesystem.  See LAST_MOUNT_STATE
    // doc.  We pass sb (the buffer we kmalloc'd here) and root_dentry
    // (set by erofs's fill_super) so the adapter can wrap them in
    // `KabiFileSystem` without walking dentry->d_sb at a guessed offset.
    *LAST_MOUNT_STATE.lock() = Some((sb as usize, s_root as usize));
    0
}

/// Initialise the kABI aops table.  Heap-allocates it and sets
/// read_folio to our synth impl.  Called once at boot from
/// kabi::init().
pub fn init_synth() {
    let aops = super::alloc::kmalloc(fl::AOPS_SIZE, 0);
    if aops.is_null() {
        log::error!("kabi: fs_synth init: kmalloc(aops) failed");
        return;
    }
    unsafe {
        core::ptr::write_bytes(aops as *mut u8, 0, fl::AOPS_SIZE);
        let aops_words = aops as *mut usize;
        *aops_words.add(fl::AOPS_READ_FOLIO_OFF / 8) =
            super::fs_synth_io::synth_read_folio as usize;
    }
    KABI_AOPS_PTR.store(aops as usize, Ordering::Release);
    log::info!(
        "kabi: fs_synth init: aops table @ {:#x}, read_folio = {:#x}",
        aops as usize,
        super::fs_synth_io::synth_read_folio as usize,
    );
}
