// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux VFS registration stubs (K33 Phase 2).
//!
//! Filesystem .ko modules call `register_filesystem(fs_type)` from
//! `init_module()` to expose themselves to the VFS.  We capture
//! the registrations in a per-arch table and surface them via
//! `lookup_fstype(name)` so `kernel/syscalls/mount.rs` can pick
//! one up at mount(2) time and call its `->mount` op.
//!
//! The actual `mount` op returns a Linux `struct super_block *`;
//! the K33 Phase 3 adapter (`fs_adapter.rs`) wraps that and
//! presents it to Kevlar's `kevlar_vfs::FileSystem` trait.
//!
//! v1 store: a small Mutex-protected `ArrayVec<FsTypeEntry, 16>`
//! — 16 is enough to hold every fs we'll bring up in K33-K34
//! (ext4, ext2, ext3, btrfs, xfs, 9p, erofs, ubifs, msdos, vfat,
//! exfat, fuseblk, ntfs3 = 13).

use core::ffi::{c_int, c_void};
use core::sync::atomic::{AtomicUsize, Ordering};

use arrayvec::ArrayVec;
use kevlar_platform::spinlock::SpinLock;

use crate::ksym;

/// One entry in the registry.  We only need the (name, fs_type
/// pointer) pair to dispatch mount(2); the fs_type's full struct
/// layout is the loaded module's concern.
#[derive(Copy, Clone)]
struct FsTypeEntry {
    /// `(*fs_type).name` — a stable C-string in the module's
    /// .rodata.  We don't take ownership; the module is never
    /// unloaded in K33.
    name_ptr: *const u8,
    /// Pointer to the module's `struct file_system_type`.  Opaque
    /// to us; we hand it back through the adapter when
    /// `kabi_mount_filesystem(name)` is called.
    fs_type: *mut c_void,
}

// SAFETY: the registry is only accessed under the lock; the
// pointers refer to stable .rodata / .bss in loaded modules.
unsafe impl Send for FsTypeEntry {}
unsafe impl Sync for FsTypeEntry {}

const MAX_FS_TYPES: usize = 16;

static FS_TYPES: SpinLock<ArrayVec<FsTypeEntry, MAX_FS_TYPES>> =
    SpinLock::new(ArrayVec::new_const());
static FS_TYPE_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Compare two zero-terminated byte strings.
unsafe fn cstr_equals(a: *const u8, b: &[u8]) -> bool {
    if a.is_null() {
        return false;
    }
    let mut i = 0usize;
    loop {
        let ca = unsafe { *a.add(i) };
        let cb = if i < b.len() { b[i] } else { 0 };
        if ca == 0 && cb == 0 {
            return true;
        }
        if ca != cb {
            return false;
        }
        i += 1;
        if i > 64 {
            return false; // sanity bound; fs names are short
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn register_filesystem(fs_type: *mut c_void) -> c_int {
    log::warn!("kabi-trace: register_filesystem({:p}) entered", fs_type);
    if fs_type.is_null() {
        return -22; // -EINVAL
    }
    // Linux's `struct file_system_type` has `name` at offset 0
    // (a `const char *`).  Read it without dereferencing the
    // module's full struct layout.
    let name_ptr = unsafe { *(fs_type as *const *const u8) };
    if name_ptr.is_null() {
        return -22;
    }
    // Quick name dump for the boot log.
    let mut name_buf = [0u8; 32];
    let mut nlen = 0;
    while nlen < 32 {
        let c = unsafe { *name_ptr.add(nlen) };
        if c == 0 {
            break;
        }
        name_buf[nlen] = c;
        nlen += 1;
    }
    let name_str = core::str::from_utf8(&name_buf[..nlen])
        .unwrap_or("<non-utf8>");
    let entry = FsTypeEntry { name_ptr, fs_type };
    {
        let mut types = FS_TYPES.lock();
        if types.try_push(entry).is_err() {
            log::warn!("kabi: register_filesystem({}): registry full",
                       name_str);
            return -12; // -ENOMEM
        }
    }
    FS_TYPE_COUNT.fetch_add(1, Ordering::Relaxed);
    log::info!("kabi: register_filesystem({}) — registered (count now {})",
               name_str,
               FS_TYPE_COUNT.load(Ordering::Relaxed));
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn unregister_filesystem(fs_type: *mut c_void) -> c_int {
    if fs_type.is_null() {
        return -22;
    }
    let mut types = FS_TYPES.lock();
    let before = types.len();
    types.retain(|e| e.fs_type != fs_type);
    let removed = before - types.len();
    if removed > 0 {
        FS_TYPE_COUNT.fetch_sub(removed, Ordering::Relaxed);
        return 0;
    }
    -2 // -ENOENT
}

ksym!(register_filesystem);
ksym!(unregister_filesystem);

/// Look up a registered filesystem by name.  Returns the opaque
/// `*mut file_system_type` pointer the module passed in.  The
/// K33 Phase 3 adapter dereferences it to call the module's
/// `->mount` op.
pub fn lookup_fstype(name: &[u8]) -> Option<*mut c_void> {
    let types = FS_TYPES.lock();
    for entry in types.iter() {
        if unsafe { cstr_equals(entry.name_ptr, name) } {
            return Some(entry.fs_type);
        }
    }
    None
}

/// Number of currently-registered filesystems.  Used by main.rs
/// boot-time logging to show kABI fs surface coverage.
pub fn registered_count() -> usize {
    FS_TYPE_COUNT.load(Ordering::Relaxed)
}
