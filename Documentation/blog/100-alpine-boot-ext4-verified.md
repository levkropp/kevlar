# Blog 100: Alpine Linux boots on Kevlar — ext4 verified, getty reached

**Date:** 2026-03-21
**Milestone:** M10 Alpine Linux

## Context

After implementing ext4 extent writes (blog 099), chown/chmod, and file
ownership propagation, the next step was to try booting a real Linux
distribution. Alpine Linux is the simplest target: BusyBox-based, no
systemd, small footprint (~8MB rootfs).

---

## ext4 Integration Test: 30/30 PASS

Before attempting Alpine, we created a comprehensive ext4+mknod integration
test (`testing/test_ext4_mknod.c`) that exercises the full ext4 write path
on a real `mkfs.ext4` disk image via virtio-blk:

| Test | Result |
|------|--------|
| Mount ext4, create/write/read extent file | PASS |
| Multi-block write (64KB, 16 blocks) | PASS |
| Read first + last block of multi-block file | PASS |
| stat() file size = 65536 | PASS |
| Truncate(0) + rewrite extent file | PASS |
| mkdir, create file in dir, readdir | PASS |
| Symlink creation + readlink | PASS |
| Unlink multi-block file (extent free) | PASS |
| rmdir | PASS |
| mknod /dev/null (major=1, minor=3) | PASS |
| Write to mknod null = discard | PASS |
| Read from mknod null = EOF | PASS |
| mknod /dev/zero (major=1, minor=5) | PASS |
| Read from mknod zero = zeros | PASS |

**Total: 30/30 PASS.** All operations on a real `mkfs.ext4` image work
correctly, including extent creation, contiguous allocation, and extent-aware
block freeing.

---

## File Ownership Propagation (Phase 7)

Extended `create_file()` and `create_dir()` trait signatures to accept
`uid: UId, gid: GId` parameters:

- 7 implementations updated (tmpfs, ext2, initramfs, cgroupfs, procfs×3)
- 7 call sites pass process credentials (`euid`/`egid`) or root (`0`/`0`)
- tmpfs: new files/dirs inherit creator's uid/gid
- ext2/ext4: new inodes written with creator's uid/gid on disk
- Kernel-internal dirs (sysfs, cgroup mounts) use root ownership

---

## Alpine Linux Boot Attempt

### Setup

Created an Alpine 3.21 rootfs from Docker (`alpine:3.21` + `openrc`),
configured for serial console, packed into a 256MB ext4 image:

```bash
docker run --name kevlar-alpine alpine:3.21 sh -c 'apk add --no-cache openrc'
docker export kevlar-alpine | tar -xf - -C build/alpine-root
# Configure inittab, clear root password, build ext4 image
mke2fs -t ext4 -d build/alpine-root build/alpine.img
```

### Boot Shim

A small C program (`testing/boot_alpine.c`) runs as PID 1 from the
initramfs. It mounts the ext4 disk, pre-mounts essential filesystems
(`/proc`, `/sys`, `/dev`, `/run`, `/tmp`) inside the new root, then
chroots and exec's `/sbin/init` (BusyBox init).

### What Works

The boot reaches this point:

```
kevlar: Alpine boot shim starting
ext4: mounted (262144 blocks, 65536 inodes, block_size=1024, inode_size=256)
kevlar: ext4 rootfs mounted on /mnt/root
kevlar: exec /sbin/init
[kevlar] sysinit: mounting filesystems
[kevlar] /dev contents:
console  full     kmsg     null     ptmx     pts
random   shm      tty      ttyS0   urandom  zero
[kevlar] sysinit complete, spawning getty
```

Breakdown of what's working:

1. **ext4 mount** from virtio-blk disk — full extent read/write
2. **chroot** into Alpine rootfs
3. **BusyBox init** reads `/etc/inittab`
4. **All sysinit commands complete:**
   - `mount -t proc proc /proc`
   - `mount -t sysfs sysfs /sys`
   - `mount -t devtmpfs devtmpfs /dev` — full device node population
   - `mkdir -p /dev/pts /dev/shm /run /tmp`
   - `mount -t tmpfs tmpfs /run` and `/tmp`
   - `hostname kevlar`
5. **All 12 device nodes** present in `/dev`
6. **Getty spawned** on ttyS0 and console

### What Fails

```
getty: ttyS0: tcsetattr: Bad file descriptor
```

Getty opens `/dev/ttyS0` successfully but `tcsetattr()` (the `TCSETS`
ioctl) fails. This is the last barrier before a login prompt.

### OpenRC Attempt

We also tried with OpenRC enabled. It gets further than expected:

```
OpenRC 0.55.1 is starting up Linux 6.19.8 (x86_64)
* Caching service dependencies ... [ ok ]
```

OpenRC starts, detects the kernel version (via `uname`), and successfully
caches service dependencies. It fails on `/run/openrc` directory creation
due to the chroot path prefix issue (OpenRC sees `/mnt/root/...` paths
instead of `/...`). Fix: implement `pivot_root` syscall.

---

## What's Needed for Login Prompt

1. **Fix `tcsetattr`/`TCSETS` ioctl** — getty needs to set terminal
   attributes. Our TTY driver likely returns the wrong error code or
   doesn't handle the ioctl path from a chrooted process correctly.
   Estimated: ~1 hour.

## What's Needed for Full Alpine Boot

1. Fix getty `tcsetattr` → login prompt works
2. Implement `pivot_root` syscall → OpenRC works (no chroot path issues)
3. A few syscalls OpenRC may need: `flock`, `statfs`, timer-related
4. Then: `apk add` for packages, networking, user management

## Path to "Build Your Own Alpine with Kevlar"

The goal: `mkfs.ext4` an image, bootstrap Alpine with `apk`, drop in
Kevlar as the kernel, boot via QEMU or real hardware (GRUB).

1. Fix getty → login works (~1 hour)
2. Fix pivot_root → OpenRC works (~2 hours)
3. Fix remaining OpenRC syscalls (~1 day)
4. Build Alpine rootfs with `apk --root` → working distro
5. Package as bootable disk image with Kevlar bzImage

---

## Summary

| Change | Impact |
|--------|--------|
| ext4 integration test | 30/30 PASS on real mkfs.ext4 image |
| File ownership (create_file/create_dir uid/gid) | New files inherit creator credentials |
| Alpine boot shim | chroot + exec /sbin/init works |
| BusyBox init sysinit | All mount/mkdir/hostname commands complete |
| devtmpfs in chroot | All 12 device nodes populated |
| Getty spawn | Reached, fails on tcsetattr — last barrier |

## Update: Alpine Proof of Life

Running Alpine's BusyBox commands via inittab sysinit lines confirms
the full userland works:

```
=========================================
  Alpine Linux 3.21 running on Kevlar!
=========================================
Linux kevlar 6.19.8 Kevlar x86_64 Linux
3.21.6

PID   USER     TIME  COMMAND
    1 root      0:01 {/sbin/init} /sbin/init
   10 root      0:00 {/bin/ps} /bin/ps

Filesystem           1K-blocks      Used Available Use% Mounted on
none                     65536     32768     32768  50% /mnt/root

bin  dev  etc  home  lib  lost+found  media  mnt  opt
proc  root  run  sbin  srv  sys  tmp  usr  var
```

Working: `uname`, `cat`, `echo`, `ls`, `ps`, `mount`, `df`, `mkdir`,
`hostname`. The full Alpine directory tree is visible from the ext4 rootfs.

Remaining issues:
- Pipe crash: `busybox | head` → SIGSEGV at 0x3d (pipe-related)
- Getty tcsetattr: respawned gettys lack inherited fds
- `/etc/os-release` empty (Docker export artifact)

**Contract tests:** 118/118 PASS
**ext4 test:** 30/30 PASS
**Alpine boot:** Commands running, userland functional
