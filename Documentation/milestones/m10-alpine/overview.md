# M10: Text-Mode Linux

**Goal:** Go from first Alpine login to Ubuntu Server–level usefulness:
writable ext4 root, real device management (udev/mdev), proper networking
(DHCP, DNS, firewall), package management (apk and apt), SSH, and
multi-user operation.

**Strategy:** Build up in layers. Alpine is the proving ground (musl,
BusyBox init, OpenRC, ~3MB rootfs). Once Alpine is solid, the same kernel
supports Ubuntu Server (glibc, systemd — already proven in M9, ext4, apt).

## Current State (Post-M9)

Working:
- 121+ syscalls, musl + glibc dynamic linking, SMP, demand paging
- BusyBox shell, procfs, devfs, sysfs, cgroupfs, ext2 (read-only)
- systemd v245 boots to target in 200ms under KVM
- TCP/UDP/Unix sockets, virtio-net, virtio-blk

Key gaps:
- No writable disk filesystem (ext2 is read-only, no ext4)
- No /dev/ttyS0 serial device, mknod is a stub
- No udev/mdev device management
- No DHCP client, DNS resolution untested from userspace
- No iptables/nftables
- chown/fchown are stubs

## Phases

| Phase | Deliverable | Est. |
|-------|-------------|------|
| 1: Alpine rootfs | Alpine minirootfs in initramfs, BusyBox init + inittab, serial getty | 2-3 days |
| 2: Interactive login | getty → login → shell on serial, TIOCSCTTY, /dev/ttyS0 | 2-3 days |
| 3: OpenRC | devfs/sysfs/hostname services, mknod, OpenRC reaches default runlevel | 3-4 days |
| 4: Writable ext4 | ext4 read-write filesystem on virtio-blk, mount as root or data disk | 2-3 weeks |
| 5: Device management | mdev or eudev, /sys device enumeration, hotplug, real mknod | 1-2 weeks |
| 6: Networking | DHCP (udhcpc), DNS, ifconfig/ip ioctls, iptables/nftables stubs | 1-2 weeks |
| 7: Multi-user + security | Real UID/GID enforcement, chown, file permissions, PAM stubs, su/sudo | 1-2 weeks |
| 8: Ubuntu Server | apt-get on Kevlar, systemd + ext4 root, SSH, docker-ready baseline | 2-3 weeks |
| **Total** | | **~3 months** |

## Phase Progression

```
Phase 1-3: Minimal Alpine login (OpenRC, serial console)
Phase 4-5: Writable disk + device management (real OS baseline)
Phase 6:   Networked server (SSH, DHCP, DNS, packages)
Phase 7:   Multi-user security (file permissions, su, sudo)
Phase 8:   Ubuntu Server equivalence (apt, systemd, docker-ready)
```
