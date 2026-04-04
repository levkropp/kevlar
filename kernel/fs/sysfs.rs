// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! sysfs — `/sys/` filesystem.
//!
//! Provides device attribute directories for `mdev -s` to scan and create
//! `/dev/` nodes dynamically. Also has empty stub directories for systemd.
use crate::fs::{
    file_system::FileSystem,
    inode::{Directory, FileLike, PollStatus},
    tmpfs::{Dir, TmpFs},
};
use crate::result::Result;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use core::cmp::min;
use core::fmt;
use kevlar_vfs::{
    inode::OpenOptions,
    stat::{FileMode, Stat, S_IFREG},
    user_buffer::{UserBufWriter, UserBufferMut},
};
use kevlar_utils::once::Once;

pub static SYS_FS: Once<Arc<SysFs>> = Once::new();

// ── SysfsFile — read-only file with dynamic content ────────────────

struct SysfsFile(String);

impl fmt::Debug for SysfsFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SysfsFile").finish()
    }
}

impl FileLike for SysfsFile {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFREG | 0o444),
            ..Stat::zeroed()
        })
    }

    fn read(&self, offset: usize, buf: UserBufferMut<'_>, _options: &OpenOptions) -> Result<usize> {
        let bytes = self.0.as_bytes();
        if offset >= bytes.len() {
            return Ok(0);
        }
        let len = min(bytes.len() - offset, buf.len());
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&bytes[offset..offset + len])?;
        Ok(len)
    }

    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus::POLLIN)
    }
}

// ── Device table ───────────────────────────────────────────────────

struct DeviceEntry {
    class: &'static str,
    name: &'static str,
    devname: &'static str,
    major: u32,
    minor: u32,
}

const CHAR_DEVICES: &[DeviceEntry] = &[
    DeviceEntry { class: "tty",  name: "ttyS0",   devname: "ttyS0",   major: 4, minor: 64 },
    DeviceEntry { class: "tty",  name: "console",  devname: "console",  major: 5, minor: 1 },
    DeviceEntry { class: "tty",  name: "tty",      devname: "tty",      major: 5, minor: 0 },
    DeviceEntry { class: "mem",  name: "null",     devname: "null",     major: 1, minor: 3 },
    DeviceEntry { class: "mem",  name: "zero",     devname: "zero",     major: 1, minor: 5 },
    DeviceEntry { class: "mem",  name: "full",     devname: "full",     major: 1, minor: 7 },
    DeviceEntry { class: "mem",  name: "random",   devname: "random",   major: 1, minor: 8 },
    DeviceEntry { class: "mem",  name: "urandom",  devname: "urandom",  major: 1, minor: 9 },
    DeviceEntry { class: "mem",  name: "kmsg",     devname: "kmsg",     major: 1, minor: 11 },
    DeviceEntry { class: "misc", name: "ptmx",     devname: "ptmx",     major: 5, minor: 2 },
];

/// Add `dev` and `uevent` files to a sysfs device directory.
fn add_dev_files(dir: &Arc<Dir>, major: u32, minor: u32, devname: &str) {
    dir.add_file(
        "dev",
        Arc::new(SysfsFile(format!("{}:{}\n", major, minor))) as Arc<dyn FileLike>,
    );
    dir.add_file(
        "uevent",
        Arc::new(SysfsFile(format!(
            "MAJOR={}\nMINOR={}\nDEVNAME={}\n",
            major, minor, devname
        ))) as Arc<dyn FileLike>,
    );
}

// ── SysFs ──────────────────────────────────────────────────────────

pub struct SysFs {
    tmpfs: TmpFs,
    class_dir: Arc<Dir>,
    block_dir: Arc<Dir>,
}

impl SysFs {
    pub fn new() -> SysFs {
        let tmpfs = TmpFs::new();
        let root = tmpfs.root_tmpfs_dir();

        // Create directories that systemd/tools expect.
        let fs = root.add_dir("fs");
        fs.add_dir("cgroup");
        let class_dir = root.add_dir("class");
        root.add_dir("devices");
        root.add_dir("bus");
        root.add_dir("kernel");
        let block_dir = root.add_dir("block");

        SysFs { tmpfs, class_dir, block_dir }
    }

    /// Populate sysfs with device entries. Called after drivers are initialized.
    pub fn populate_devices(&self) {
        // Create class sub-directories.
        let tty_dir = self.class_dir.add_dir("tty");
        let mem_dir = self.class_dir.add_dir("mem");
        let misc_dir = self.class_dir.add_dir("misc");
        let net_dir = self.class_dir.add_dir("net");

        // Character devices.
        for dev in CHAR_DEVICES {
            let parent = match dev.class {
                "tty" => &tty_dir,
                "mem" => &mem_dir,
                "misc" => &misc_dir,
                _ => continue,
            };
            let dev_dir = parent.add_dir(dev.name);
            add_dev_files(&dev_dir, dev.major, dev.minor, dev.devname);
        }

        // Framebuffer device: /sys/class/graphics/fb0
        // Xorg's fbdev driver probes this path to discover framebuffers.
        if bochs_fb::is_initialized() {
            let graphics_dir = self.class_dir.add_dir("graphics");
            let fb0_dir = graphics_dir.add_dir("fb0");
            add_dev_files(&fb0_dir, 29, 0, "fb0");
        }

        // Block device: virtio-blk (major 253, minor 0).
        if kevlar_api::driver::block::block_device().is_some() {
            let vda_dir = self.block_dir.add_dir("vda");
            add_dev_files(&vda_dir, 253, 0, "vda");
        }

        // Network interface (no dev file — net devices don't get /dev/ nodes).
        net_dir.add_dir("eth0");
    }
}

impl FileSystem for SysFs {
    fn root_dir(&self) -> Result<Arc<dyn Directory>> {
        self.tmpfs.root_dir()
    }
}

pub fn init() {
    SYS_FS.init(|| Arc::new(SysFs::new()));
}

/// Populate sysfs with device entries. Must be called after drivers are initialized.
pub fn populate() {
    SYS_FS.populate_devices();
}
