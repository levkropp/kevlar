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

// ── SysfsBinary — read-only binary file ───────────────────────────

struct SysfsBinary(alloc::vec::Vec<u8>);

impl fmt::Debug for SysfsBinary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SysfsBinary").finish()
    }
}

impl FileLike for SysfsBinary {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFREG | 0o444),
            size: kevlar_vfs::stat::FileSize(self.0.len() as isize),
            ..Stat::zeroed()
        })
    }

    fn read(&self, offset: usize, buf: UserBufferMut<'_>, _options: &OpenOptions) -> Result<usize> {
        if offset >= self.0.len() {
            return Ok(0);
        }
        let len = min(self.0.len() - offset, buf.len());
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&self.0[offset..offset + len])?;
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
    pci_devices: Arc<Dir>,
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
        let bus_dir = root.add_dir("bus");
        // /sys/bus/pci/devices/ — Xorg scans this for GPU detection
        let pci_bus = bus_dir.add_dir("pci");
        let pci_devices = pci_bus.add_dir("devices");
        root.add_dir("kernel");
        let block_dir = root.add_dir("block");

        SysFs { tmpfs, class_dir, block_dir, pci_devices }
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

            // PCI device entry — Xorg scans /sys/bus/pci/devices/ for VGA class.
            let pci_dev = self.pci_devices.add_dir("0000:00:02.0");
            pci_dev.add_file("vendor", Arc::new(SysfsFile("0x1234\n".into())) as Arc<dyn FileLike>);
            pci_dev.add_file("device", Arc::new(SysfsFile("0x1111\n".into())) as Arc<dyn FileLike>);
            pci_dev.add_file("class", Arc::new(SysfsFile("0x030000\n".into())) as Arc<dyn FileLike>);
            pci_dev.add_file("subsystem_vendor", Arc::new(SysfsFile("0x1af4\n".into())) as Arc<dyn FileLike>);
            pci_dev.add_file("subsystem_device", Arc::new(SysfsFile("0x1100\n".into())) as Arc<dyn FileLike>);
            // resource: BAR0 at the framebuffer physical address
            let bar0_addr = bochs_fb::phys_addr();
            let bar0_end = bar0_addr + bochs_fb::size() - 1;
            let resource_str = alloc::format!(
                "{:#018x} {:#018x} 0x0000000000040200\n", bar0_addr, bar0_end
            );
            pci_dev.add_file("resource", Arc::new(SysfsFile(resource_str)) as Arc<dyn FileLike>);
            // config: raw 256-byte PCI config space (libpciaccess reads this)
            let mut config = [0u8; 256];
            // Vendor ID (0x1234) at offset 0
            config[0] = 0x34; config[1] = 0x12;
            // Device ID (0x1111) at offset 2
            config[2] = 0x11; config[3] = 0x11;
            // Command: I/O + Memory + Bus Master at offset 4
            config[4] = 0x07; config[5] = 0x00;
            // Status at offset 6
            config[6] = 0x00; config[7] = 0x00;
            // Revision at offset 8
            config[8] = 0x05;
            // Class code: VGA (0x030000) at offsets 9-11
            config[9] = 0x00;  // prog_if
            config[10] = 0x00; // subclass (VGA)
            config[11] = 0x03; // class (Display)
            // BAR0 at offset 0x10 (framebuffer physical address)
            let bar0_bytes = (bar0_addr as u32).to_le_bytes();
            config[0x10..0x14].copy_from_slice(&bar0_bytes);
            // BAR2 at offset 0x18 (Bochs VBE MMIO)
            config[0x18] = 0x00; config[0x19] = 0x00;
            config[0x1a] = 0x00; config[0x1b] = 0x00;
            // Subsystem vendor/device at offsets 0x2c-0x2f
            config[0x2c] = 0xf4; config[0x2d] = 0x1a; // 0x1af4
            config[0x2e] = 0x00; config[0x2f] = 0x11; // 0x1100
            pci_dev.add_file("config", Arc::new(SysfsBinary(config.to_vec())) as Arc<dyn FileLike>);
            // Additional files libpciaccess/Xorg reads:
            pci_dev.add_file("irq", Arc::new(SysfsFile("10\n".into())) as Arc<dyn FileLike>);
            pci_dev.add_file("numa_node", Arc::new(SysfsFile("-1\n".into())) as Arc<dyn FileLike>);
            pci_dev.add_file("enable", Arc::new(SysfsFile("1\n".into())) as Arc<dyn FileLike>);
            pci_dev.add_file("broken_parity_status", Arc::new(SysfsFile("0\n".into())) as Arc<dyn FileLike>);
            pci_dev.add_file("boot_vga", Arc::new(SysfsFile("1\n".into())) as Arc<dyn FileLike>);
            pci_dev.add_file("revision", Arc::new(SysfsFile("0x05\n".into())) as Arc<dyn FileLike>);
            // Cross-link: /sys/bus/pci/devices/0000:00:02.0/graphics/fb0
            // Xorg uses this to map PCI device → framebuffer device.
            let pci_gfx = pci_dev.add_dir("graphics");
            let pci_fb0 = pci_gfx.add_dir("fb0");
            add_dev_files(&pci_fb0, 29, 0, "fb0");
        }

        // Input devices: /sys/class/input/eventN — major 13, minor
        // 64+N.  libinput-style discovery walks this directory to
        // find evdev devices.  Listing the entries even without a
        // udev daemon is enough for static-config Xorg evdev to
        // work, since we point it at /dev/input/eventN explicitly.
        {
            let input_class = self.class_dir.add_dir("input");
            for (i, dev) in virtio_input::registered_devices().iter().enumerate() {
                let name = alloc::format!("event{}", i);
                let dir = input_class.add_dir(&name);
                add_dev_files(&dir, 13, (64 + i) as u32, &name);
                let dev_name = dev.name.lock().clone();
                dir.add_file(
                    "name",
                    Arc::new(SysfsFile(alloc::format!("{}\n", dev_name)))
                        as Arc<dyn FileLike>,
                );
            }
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
