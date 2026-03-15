# M10 Phase 5: Device Management

**Goal:** Dynamic device discovery and `/dev/` node creation via
mdev or eudev, driven by sysfs enumeration.

## Why This Matters

Real Linux systems don't hardcode `/dev/` entries. They scan `/sys/`
for devices and create nodes dynamically. Alpine uses `mdev` (BusyBox),
Ubuntu uses `udev` (systemd). Both need:

1. `/sys/` populated with device attributes (bus, class, vendor, etc.)
2. Kernel uevent mechanism or `/proc/sys/kernel/hotplug` for notifications
3. Real `mknod()` to create character/block device nodes

## Scope

### sysfs device tree

Currently our `/sys/` has empty stub directories. Need:

- `/sys/class/tty/ttyS0` → device attributes (dev major:minor)
- `/sys/class/block/vda` → block device attributes
- `/sys/class/net/eth0` → network device attributes
- `/sys/bus/pci/devices/` → PCI device enumeration
- `/sys/devices/` → device hierarchy

Each device directory contains:
- `dev` file → "major:minor\n" (e.g., "4:64\n" for ttyS0)
- `uevent` file → "MAJOR=4\nMINOR=64\nDEVNAME=ttyS0\n"
- `subsystem` symlink → `../../class/tty`

### mknod implementation

Real `mknod(path, mode, dev)`:
- Create a device inode in the filesystem (tmpfs)
- `mode` encodes type: `S_IFCHR` (char) or `S_IFBLK` (block)
- `dev` encodes major:minor via `makedev(major, minor)`
- When the device node is opened, dispatch to the correct driver
  based on major:minor number

### Device major:minor registry

Map major:minor to driver:

| Major | Minor | Device |
|-------|-------|--------|
| 1 | 3 | /dev/null |
| 1 | 5 | /dev/zero |
| 1 | 8 | /dev/random |
| 1 | 9 | /dev/urandom |
| 4 | 1-63 | /dev/tty1-63 (virtual consoles) |
| 4 | 64+ | /dev/ttyS0+ (serial ports) |
| 5 | 0 | /dev/tty (current tty) |
| 5 | 1 | /dev/console |
| 5 | 2 | /dev/ptmx |
| 253 | 0+ | /dev/vda+ (virtio-blk) |

### Hotplug notification

Linux uses netlink (KOBJECT_UEVENT) or `/proc/sys/kernel/hotplug`.
mdev uses the hotplug mechanism:
1. Kernel writes device path to `/proc/sys/kernel/hotplug`
2. On device add/remove, kernel execs the hotplug binary
3. mdev reads environment variables (ACTION, DEVPATH, SUBSYSTEM)

For initial support, pre-populate `/dev/` at boot and skip hotplug.
Add hotplug later for USB/disk hot-add.

## Verification

```
mdev -s  # scan /sys and populate /dev
ls -la /dev/ttyS0 /dev/vda /dev/null  # all exist with correct major:minor
cat /sys/class/tty/ttyS0/dev  # prints "4:64"
```
