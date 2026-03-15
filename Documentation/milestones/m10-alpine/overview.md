# M10: Alpine Linux (Text Mode)

**Goal:** Boot Alpine Linux to an interactive login prompt on the serial
console, with OpenRC service management and apk package installation.

**Why Alpine?** Alpine uses musl libc (already tested on Kevlar), BusyBox
init (simpler than systemd), and OpenRC (shell-script based — no D-Bus).
The minirootfs is 2.7MB. It's the natural next step after systemd on
glibc, proving Kevlar works with both major C libraries and init systems.

## Current State (Post-M9)

Working:
- 121+ syscalls, musl + glibc dynamic linking, SMP, demand paging
- BusyBox shell, procfs, devfs, sysfs, cgroupfs, ext2 (read-only)
- systemd v245 boots to target in 200ms under KVM
- TCP/UDP/Unix sockets, virtio-net, virtio-blk

Key gaps for Alpine:
- No writable root filesystem (initramfs is read-only)
- No /dev/ttyS0 serial device node
- mknod is a stub (doesn't create real device nodes)
- TIOCSCTTY ioctl may be incomplete
- chown/fchown are stubs

## Phases

| Phase | Deliverable | Est. |
|-------|-------------|------|
| 1: Alpine rootfs | Alpine minirootfs in initramfs, BusyBox init boots with inittab | 2-3 days |
| 2: getty + login | Serial console getty, login, interactive shell | 2-3 days |
| 3: OpenRC | devfs/sysfs/hostname services, OpenRC reaches default runlevel | 3-4 days |
| 4: Networking + apk | apk add works, dropbear SSH, network config | 3-4 days |
| **Total** | | **~2 weeks** |
