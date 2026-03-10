// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Minimal sysfs stub.
//!
//! Provides empty /sys/fs/cgroup, /sys/class, /sys/devices directories.
//! systemd probes sysfs extensively but survives with empty content.
use crate::fs::{file_system::FileSystem, inode::Directory, tmpfs::TmpFs};
use crate::result::Result;
use alloc::sync::Arc;
use kevlar_utils::once::Once;

pub static SYS_FS: Once<Arc<SysFs>> = Once::new();

pub struct SysFs(TmpFs);

impl SysFs {
    pub fn new() -> SysFs {
        let tmpfs = TmpFs::new();
        let root = tmpfs.root_tmpfs_dir();

        // Create empty directories that systemd expects.
        let fs = root.add_dir("fs");
        fs.add_dir("cgroup");
        root.add_dir("class");
        root.add_dir("devices");
        root.add_dir("bus");
        root.add_dir("kernel");

        SysFs(tmpfs)
    }
}

impl FileSystem for SysFs {
    fn root_dir(&self) -> Result<Arc<dyn Directory>> {
        self.0.root_dir()
    }
}

pub fn init() {
    SYS_FS.init(|| Arc::new(SysFs::new()));
}
