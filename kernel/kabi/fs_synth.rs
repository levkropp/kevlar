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
