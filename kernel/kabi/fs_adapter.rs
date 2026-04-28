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

/// Mount entry point — looks up the registered filesystem by name,
/// dispatches into its `->mount` op (or init_fs_context/get_tree
/// chain), and wraps the result in a `KabiFileSystem` adapter.
///
/// v1: returns `Err(ENOSYS)` after lookup succeeds.  The lookup
/// itself proves the registry contains the fs (Phase 2c
/// achievement); the mount-op dispatch is Phase 3b.
pub fn kabi_mount_filesystem(
    name: &str,
    _source: Option<&str>,
    _flags: u32,
    _data: *const u8,
) -> Result<Arc<dyn FileSystem>> {
    // Look up the registered file_system_type.  If not registered,
    // return -ENODEV so mount.rs can fall back to other handlers.
    let name_bytes = name.as_bytes();
    let _fs_type = match super::fs_register::lookup_fstype(name_bytes) {
        Some(p) => p,
        None => {
            warn!("kabi: kabi_mount_filesystem({}): not in fs registry", name);
            return Err(crate::result::Error::new(
                crate::result::Errno::ENODEV,
            ));
        }
    };

    // TODO Phase 3b: read fs_type's `->mount` (offset 32) or
    // `->init_fs_context` (offset 16); dispatch.
    log::warn!(
        "kabi: kabi_mount_filesystem({}): registry hit, dispatch to \
         module ->mount op not yet implemented",
        name,
    );
    Err(crate::result::Error::new(crate::result::Errno::ENOSYS))
}
