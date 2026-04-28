// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Adapter from a Linux `struct super_block *` (returned by a kABI-
//! loaded filesystem `.ko` module) to Kevlar's
//! `kevlar_vfs::FileSystem` trait.  K33 Phase 3.
//!
//! Two-stage handshake:
//!
//!   1.  `kabi_mount_filesystem(name, source, flags, data)` looks up
//!       the registered `*file_system_type` and dispatches into the
//!       module's `->mount` op (or, for modern fs's that use
//!       `init_fs_context` + `get_tree`, the fs_context dance).
//!       Returns a Linux `super_block *`.
//!
//!   2.  `KabiFileSystem` wraps that super_block and implements
//!       `kevlar_vfs::FileSystem::root_dir()` by walking
//!       `sb->s_root->d_inode->i_op->lookup` etc.
//!
//! v1 (this commit): the adapter exists with the right type
//! shape but `kabi_mount_filesystem` returns `Err(ENOSYS)` because
//! we don't yet have:
//!
//!   * Real `bdev_file_open_by_path` (synthetic block_device
//!     wrapping our virtio_blk).
//!   * Real `get_tree_bdev_flags` (the modern mount trampoline
//!     that erofs uses via `init_fs_context`).
//!   * The page-cache backing for `read_folio` ops.
//!
//! Each of those is a separate Phase 3b commit.  The point of v1
//! is to land the routing layer + adapter struct so Phase 3b can
//! drop in real impls without touching mount.rs.

use alloc::sync::Arc;
use core::ffi::c_void;

use kevlar_vfs::file_system::FileSystem;
use kevlar_vfs::inode::Directory;
use kevlar_vfs::result::{Errno, Error, Result as VfsResult};

use crate::prelude::*;

/// `KabiFileSystem` wraps a `*mut super_block` returned by a Linux
/// fs `.ko` module's `->mount` op (or its `init_fs_context` →
/// `get_tree` chain).  It implements Kevlar's `FileSystem` trait
/// by translating `root_dir()` calls into a walk through the
/// Linux super_block's root dentry.
#[derive(Debug)]
pub struct KabiFileSystem {
    /// `*mut super_block` from Linux's perspective.  Opaque to us;
    /// dereferenced only through helper fns that know the offsets.
    super_block: *mut c_void,
    /// Filesystem name for diagnostics (e.g. "erofs").
    name: alloc::string::String,
}

// SAFETY: super_block accesses go through the kABI surface which
// itself synchronises.  v1 v never accesses the super_block (root_dir
// returns ENOSYS); Send + Sync are needed to satisfy FileSystem.
unsafe impl Send for KabiFileSystem {}
unsafe impl Sync for KabiFileSystem {}

impl KabiFileSystem {
    pub fn new(super_block: *mut c_void, name: alloc::string::String) -> Self {
        KabiFileSystem { super_block, name }
    }
}

impl FileSystem for KabiFileSystem {
    fn root_dir(&self) -> VfsResult<Arc<dyn Directory>> {
        log::warn!(
            "kabi: KabiFileSystem({}).root_dir() — not yet implemented \
             (super_block={:p}); Phase 3b lands the dentry walk",
            self.name, self.super_block,
        );
        Err(Error::new(Errno::ENOSYS))
    }
}

/// Linux 7.0.0-14 `struct file_system_type` layout (the fields we
/// care about; the full struct is ~120 bytes).  Pinned to the
/// version we load (Ubuntu 26.04's linux-modules-7.0.0-14-generic).
///
/// ```c
/// struct file_system_type {
///     const char *name;                  /* +0 */
///     int fs_flags;                      /* +8 */
///     int (*init_fs_context)(struct fs_context *);  /* +16 */
///     const struct fs_parameter_spec *parameters;   /* +24 */
///     struct dentry *(*mount)(struct file_system_type *, int,
///                             const char *, void *); /* +32 */
///     void (*kill_sb)(struct super_block *);         /* +40 */
///     struct module *owner;                           /* +48 */
///     ...
/// };
/// ```
const FST_NAME_OFF: usize = 0;
const FST_FS_FLAGS_OFF: usize = 8;
const FST_INIT_FS_CONTEXT_OFF: usize = 16;
const FST_MOUNT_OFF: usize = 32;
const FST_KILL_SB_OFF: usize = 40;

/// Mount entry point — looks up the registered filesystem by name,
/// reads its struct file_system_type, dispatches into its mount
/// pathway, and wraps the result in a `KabiFileSystem` adapter.
///
/// Phase 3 v1 (042f57a): registry lookup proven; mount-op dispatch
/// returned ENOSYS unconditionally.
///
/// Phase 3b v1 (this commit): inspect file_system_type fields and
/// log them.  If `->mount` is non-null, dispatch into it directly
/// (older fs's like ext2 still use this path).  If only
/// `->init_fs_context` is set (modern erofs/ext4/btrfs), log that
/// the init_fs_context chain isn't implemented yet and return
/// ENOSYS — Phase 3c lands the chain.
pub fn kabi_mount_filesystem(
    name: &str,
    _source: Option<&str>,
    _flags: u32,
    _data: *const u8,
) -> Result<Arc<dyn FileSystem>> {
    let name_bytes = name.as_bytes();
    let fs_type = match super::fs_register::lookup_fstype(name_bytes) {
        Some(p) => p,
        None => {
            warn!("kabi: kabi_mount_filesystem({}): not in fs registry", name);
            return Err(crate::result::Error::new(
                crate::result::Errno::ENODEV,
            ));
        }
    };

    // Read the struct fields without dereferencing the full layout.
    let fs_type_u8 = fs_type as *const u8;
    let stored_name_ptr =
        unsafe { *(fs_type_u8.add(FST_NAME_OFF) as *const *const u8) };
    let fs_flags = unsafe { *(fs_type_u8.add(FST_FS_FLAGS_OFF) as *const i32) };
    let init_fs_context_ptr =
        unsafe { *(fs_type_u8.add(FST_INIT_FS_CONTEXT_OFF) as *const usize) };
    let mount_op_ptr =
        unsafe { *(fs_type_u8.add(FST_MOUNT_OFF) as *const usize) };
    let kill_sb_ptr =
        unsafe { *(fs_type_u8.add(FST_KILL_SB_OFF) as *const usize) };

    // Read up to 16 bytes of the registered name string for diagnostics.
    let mut stored_name_buf = [0u8; 16];
    if !stored_name_ptr.is_null() {
        for i in 0..16 {
            let c = unsafe { *stored_name_ptr.add(i) };
            stored_name_buf[i] = c;
            if c == 0 { break; }
        }
    }
    let stored_name = core::str::from_utf8(&stored_name_buf)
        .unwrap_or("<non-utf8>")
        .trim_end_matches('\0');

    info!(
        "kabi: file_system_type({}): name=\"{}\" fs_flags={:#x} \
         init_fs_context={:#x} mount={:#x} kill_sb={:#x}",
        name, stored_name, fs_flags,
        init_fs_context_ptr, mount_op_ptr, kill_sb_ptr,
    );

    if mount_op_ptr != 0 {
        // Type-shape the dispatch.  We do NOT yet actually call the
        // function pointer — first attempt (Phase 3c v0) crashed with
        // a kernel page fault four instructions into erofs's
        // ->mount thunk:
        //
        //   panic at platform/arm64/interrupt.rs:136:17:
        //   kernel page fault: pc=0xffff00007cdc2c1c
        //                      far=0x2820262029766574 (= "tev)&( (")
        //                      esr=0x96000004
        //
        // The FAR value is ASCII text, suggesting the thunk reads a
        // string-typed field that we haven't initialized.  Likely
        // candidates: the kernel's `mount_bdev` helper (which most
        // legacy mount thunks delegate to) reads `current->fs->...`
        // or similar process-context state we don't model.
        //
        // Phase 3d will either (a) implement a real synthetic
        // block_device wrapping virtio_blk + the process-context
        // state mount_bdev needs, or (b) switch to the
        // init_fs_context path which doesn't go through mount_bdev.
        // Until then, log the dispatch shape and bail.
        type MountFn = unsafe extern "C" fn(
            fs_type: *mut c_void,
            flags: i32,
            dev_name: *const u8,
            data: *mut c_void,
        ) -> *mut c_void;
        let _mount_fn: MountFn = unsafe { core::mem::transmute(mount_op_ptr) };

        warn!(
            "kabi: erofs ->mount at {:#x} — call gated off until Phase 3d \
             provides synthetic block_device + process-context backing \
             (v0 dispatch panicked at PC+0x24 with text-shaped FAR)",
            mount_op_ptr,
        );
        return Err(crate::result::Error::new(crate::result::Errno::ENOSYS));
    }
    if init_fs_context_ptr != 0 {
        warn!(
            "kabi: file_system_type({}) uses init_fs_context — fs_context \
             dispatch not yet implemented (Phase 3c)",
            name,
        );
        return Err(crate::result::Error::new(crate::result::Errno::ENOSYS));
    }
    warn!(
        "kabi: file_system_type({}) has neither ->mount nor \
         ->init_fs_context — module is malformed?",
        name,
    );
    Err(crate::result::Error::new(crate::result::Errno::EINVAL))
}
