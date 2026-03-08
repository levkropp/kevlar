// SPDX-License-Identifier: MIT OR Apache-2.0
use super::inode::Directory;
use crate::result::Result;
use alloc::sync::Arc;

pub trait FileSystem: Send + Sync {
    fn root_dir(&self) -> Result<Arc<dyn Directory>>;
}
