// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use super::inode::Directory;
use crate::result::Result;
use alloc::sync::Arc;

/// A mountable filesystem (Ring 2 service boundary).
///
/// Filesystem services implement this trait and register with the VFS Core.
/// In Phase 4, calls through this trait will be wrapped in `catch_unwind`
/// for panic containment, allowing a faulty filesystem to return `EIO`
/// instead of crashing the kernel.
pub trait FileSystem: Send + Sync {
    fn root_dir(&self) -> Result<Arc<dyn Directory>>;
}
