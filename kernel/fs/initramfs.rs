// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Re-export initramfs from the kevlar_initramfs service crate.
//!
//! Only `init()` stays here because it uses `include_bytes!` with the
//! build-time `INITRAMFS_PATH` environment variable.
pub use kevlar_initramfs::*;

use alloc::sync::Arc;

pub fn init() {
    INITRAM_FS.init(|| {
        let image = include_bytes!(concat!("../../", env!("INITRAMFS_PATH")));
        if image.is_empty() {
            panic!("initramfs is not embedded");
        }

        Arc::new(InitramFs::new(image))
    });
}
