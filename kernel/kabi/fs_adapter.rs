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

/// Linux 7.0.0-14 `struct file_system_type` layout, sourced
/// directly from `include/linux/fs.h:2271` of the matching
/// kernel tree (`build/linux-src.vanilla-v7.0/`):
///
/// ```c
/// struct file_system_type {
///     const char *name;                            /* +0  */
///     int fs_flags;                                /* +8  */
///     int (*init_fs_context)(struct fs_context *); /* +16 */
///     const struct fs_parameter_spec *parameters;  /* +24 */
///     void (*kill_sb)(struct super_block *);       /* +32 */
///     struct module *owner;                        /* +40 */
///     struct file_system_type *next;               /* +48 */
///     ...
/// };
/// ```
///
/// Important: **Linux 7.0 has no legacy `->mount` field.**  Earlier
/// Linux kernels (≤ 6.x) had `mount` at +32 and `kill_sb` at +40;
/// 7.0 removed `mount` and shifted `kill_sb` up.  The Phase 3c v0
/// crash (calling +32 as if it were `mount`) was actually a call
/// into erofs's kill_sb thunk with the wrong argument types, which
/// promptly dereferenced a string-shaped value.  Fixed by routing
/// through init_fs_context instead.
const FST_NAME_OFF: usize = 0;
const FST_FS_FLAGS_OFF: usize = 8;
const FST_INIT_FS_CONTEXT_OFF: usize = 16;
const FST_KILL_SB_OFF: usize = 32;

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
         init_fs_context={:#x} kill_sb={:#x}",
        name, stored_name, fs_flags, init_fs_context_ptr, kill_sb_ptr,
    );

    if init_fs_context_ptr == 0 {
        warn!(
            "kabi: file_system_type({}) has no init_fs_context — \
             pre-Linux-7.0 module style not yet supported",
            name,
        );
        return Err(crate::result::Error::new(crate::result::Errno::ENOSYS));
    }

    // Modern mount path: init_fs_context → parse_param → get_tree.
    //
    // Linux 7.0 removed the legacy `->mount` callback entirely;
    // every fs uses init_fs_context.  Our v1 implementation:
    //
    //  1. allocate a `struct fs_context` (zero-filled, ~256 bytes).
    //  2. populate fs_context.fs_type = our registered fs_type_ptr,
    //     fs_context.purpose = FS_CONTEXT_FOR_MOUNT (1),
    //     fs_context.sb_flags = MS_RDONLY-ish.
    //  3. call init_fs_context(fc) — fills fc->ops with the fs's
    //     parse_param / get_tree ops, plus fc->fs_private with the
    //     fs-specific context.
    //  4. parse_param loop (skip — no mount options).
    //  5. fc->ops->get_tree(fc) — does the real work, returns
    //     fc->root populated with the mounted dentry.
    //
    // get_tree for block-fs's resolves through bdev_file_open_by_path.
    // Our stub returns ERR_PTR(-ENODEV) which causes get_tree to
    // bail cleanly with -ENODEV — no crash.
    //
    // Phase 3d v1 (this commit): set up steps 1-3 only.  parse_param
    // and get_tree are Phase 3d v2; calling get_tree without a real
    // bdev backing means we will fail at the device-open step but
    // at least the context allocation + init_fs_context call should
    // succeed.

    type InitFsContextFn = unsafe extern "C" fn(*mut c_void) -> i32;
    let init_fc_fn: InitFsContextFn =
        unsafe { core::mem::transmute(init_fs_context_ptr) };

    // struct fs_context is ~280 bytes in Linux 7.0; allocate 512 to
    // be safe.  The fields we know we need to touch:
    //   +0   const struct fs_context_operations *ops
    //   +24  struct file_system_type *fs_type
    //   +56  void *fs_private
    //   +96  unsigned int sb_flags
    //   +112 enum fs_context_purpose purpose
    let fc = super::alloc::kmalloc(512, 0);
    if fc.is_null() {
        return Err(crate::result::Error::new(crate::result::Errno::ENOMEM));
    }
    // Zero-fill the buffer so init_fs_context sees clean defaults.
    unsafe { core::ptr::write_bytes(fc as *mut u8, 0, 512); }

    // Field offsets sourced from include/linux/fs_context.h.
    //
    //   struct fs_context {
    //       const struct fs_context_operations *ops;  // +0   size 8
    //       struct mutex uapi_mutex;                  // +8   size 32 (no DEBUG)
    //       struct file_system_type *fs_type;         // +40  size 8
    //       void *fs_private;                         // +48  size 8
    //       void *sget_key;                           // +56  size 8
    //       struct dentry *root;                      // +64  size 8
    //       struct user_namespace *user_ns;           // +72  size 8
    //       struct net *net_ns;                       // +80  size 8
    //       const struct cred *cred;                  // +88  size 8
    //       struct p_log log;                         // +96  size 16 (2 ptrs)
    //       const char *source;                       // +112 size 8
    //       ...
    //       unsigned int sb_flags;                    // +128
    //       unsigned int sb_flags_mask;               // +132
    //       unsigned int s_iflags;                    // +136
    //       enum fs_context_purpose purpose:8;        // +140 (bitfield)
    //       enum fs_context_phase phase:8;            //
    //   }
    use super::struct_layouts as fl;
    const FS_CONTEXT_FOR_MOUNT: u8 = 1;
    const SB_RDONLY: u32 = 1;

    // Source string lives in module-private memory; we kmalloc'd
    // copy so erofs can dereference it without thinking about
    // lifetime.  Phase 3e: point at the embedded test image
    // (mkfs.erofs-built; see tools/build-initramfs.py).  When
    // filp_open is implemented (Phase 3f) it will open this path
    // through Kevlar's initramfs lookup and erofs's fc_fill_super
    // will then read the on-disk superblock.
    const SOURCE_PATH: &[u8] = b"/lib/test.erofs\0";
    let source_buf = super::alloc::kmalloc(SOURCE_PATH.len(), 0) as *mut u8;
    if source_buf.is_null() {
        super::alloc::kfree(fc);
        return Err(crate::result::Error::new(crate::result::Errno::ENOMEM));
    }
    unsafe {
        core::ptr::copy_nonoverlapping(
            SOURCE_PATH.as_ptr(), source_buf, SOURCE_PATH.len(),
        );
    }

    unsafe {
        *(fc.cast::<u8>().add(fl::FC_FS_TYPE_OFF) as *mut *mut c_void) = fs_type;
        *(fc.cast::<u8>().add(fl::FC_SOURCE_OFF) as *mut *const u8) = source_buf;
        *(fc.cast::<u8>().add(fl::FC_SB_FLAGS_OFF) as *mut u32) = SB_RDONLY;
        *(fc.cast::<u8>().add(fl::FC_PURPOSE_OFF) as *mut u8) =
            FS_CONTEXT_FOR_MOUNT;
    }

    info!(
        "kabi: dispatching erofs init_fs_context(fc={:p})",
        fc,
    );
    let rc = unsafe { init_fc_fn(fc) };

    if rc < 0 {
        warn!(
            "kabi: erofs init_fs_context returned {} — bailing",
            rc,
        );
        super::alloc::kfree(fc);
        let errno = match -rc {
            12 => crate::result::Errno::ENOMEM,
            22 => crate::result::Errno::EINVAL,
            _  => crate::result::Errno::EIO,
        };
        return Err(crate::result::Error::new(errno));
    }

    info!("kabi: erofs init_fs_context returned 0 — fc->ops populated");

    // ── Phase 3d v2: read fc->ops, invoke ops->get_tree(fc) ────────
    //
    // After init_fs_context succeeds, fc->ops at fs_context offset 0
    // holds a pointer to the fs's struct fs_context_operations.
    // Layout (include/linux/fs_context.h:115):
    //
    //   struct fs_context_operations {
    //       void (*free)(...);            /* +0  */
    //       int  (*dup)(...);             /* +8  */
    //       int  (*parse_param)(...);     /* +16 */
    //       int  (*parse_monolithic)(...); /* +24 */
    //       int  (*get_tree)(...);        /* +32 */
    //       int  (*reconfigure)(...);     /* +40 */
    //   };

    const FC_OPS_OFF: usize = 0;
    const OPS_GET_TREE_OFF: usize = 32;

    let ops_ptr = unsafe { *(fc.cast::<u8>().add(FC_OPS_OFF) as *const usize) };
    if ops_ptr == 0 {
        warn!("kabi: erofs init_fs_context didn't populate fc->ops");
        super::alloc::kfree(fc);
        return Err(crate::result::Error::new(crate::result::Errno::EIO));
    }
    info!("kabi: fc->ops = {:#x}", ops_ptr);

    let get_tree_ptr = unsafe {
        *((ops_ptr as *const u8).add(OPS_GET_TREE_OFF) as *const usize)
    };
    if get_tree_ptr == 0 {
        warn!("kabi: fc->ops->get_tree is null");
        super::alloc::kfree(fc);
        return Err(crate::result::Error::new(crate::result::Errno::EIO));
    }
    info!("kabi: fc->ops->get_tree = {:#x}", get_tree_ptr);

    type GetTreeFn = unsafe extern "C" fn(*mut c_void) -> i32;
    let get_tree_fn: GetTreeFn = unsafe { core::mem::transmute(get_tree_ptr) };

    info!("kabi: dispatching erofs ops->get_tree(fc={:p})", fc);
    let rc = unsafe { get_tree_fn(fc) };

    if rc < 0 {
        warn!(
            "kabi: erofs ops->get_tree returned {} — expected for v2 \
             (no real block device behind bdev_file_open_by_path)",
            rc,
        );
        super::alloc::kfree(fc);
        let errno = match -rc {
            12 => crate::result::Errno::ENOMEM,
            16 => crate::result::Errno::EBUSY,
            19 => crate::result::Errno::ENODEV,
            22 => crate::result::Errno::EINVAL,
            _  => crate::result::Errno::EIO,
        };
        return Err(crate::result::Error::new(errno));
    }

    // Success path: fc->root holds the mounted dentry.  Wrap and
    // return.  Phase 3d v3 will walk the dentry to provide
    // root_dir() backing.
    info!(
        "kabi: erofs ops->get_tree returned 0 — fc->root populated, \
         dentry walk is Phase 3d v3",
    );
    Err(crate::result::Error::new(crate::result::Errno::ENOSYS))
}
