# M10 Phase 1: Alpine Rootfs in Initramfs

**Goal:** BusyBox init reads `/etc/inittab` and spawns getty on serial.

## Approach

Download `alpine-minirootfs-3.21.3-x86_64.tar.gz` (~2.7MB) and extract
it into the Dockerfile's final stage. This gives us a complete Alpine
userland: BusyBox (dynamically linked to musl), `/etc/passwd`,
`/etc/shadow`, `/etc/group`, apk configuration.

We keep our existing statically-linked BusyBox as a fallback at
`/bin/busybox-static` but use Alpine's dynamically-linked BusyBox as
the primary `/bin/busybox`. This tests musl dynamic linking with the
real distro binary.

## Inittab

Minimal inittab for serial console boot:

```
::sysinit:/bin/mount -t proc proc /proc
::sysinit:/bin/mount -t sysfs sysfs /sys
::sysinit:/bin/mount -t tmpfs tmpfs /run
::sysinit:/bin/mkdir -p /dev/pts /dev/shm
::sysinit:/bin/mount -t devpts devpts /dev/pts
::sysinit:/bin/hostname -F /etc/hostname
ttyS0::respawn:/sbin/getty -L 115200 ttyS0 vt100
::ctrlaltdel:/sbin/reboot
::shutdown:/bin/umount -a -r
```

This skips OpenRC for Phase 1 and uses BusyBox init's built-in
inittab processing directly. Phase 3 adds OpenRC.

## Kernel Changes

- `/dev/ttyS0` device node — maps to our serial TTY (same physical
  device as `/dev/console`, exposed as a named serial port)
- Writable tmpfs overlay — BusyBox init needs to write to `/var/run/`
  and create PID files. Mount tmpfs at `/run` and `/tmp` during boot.
  Our initramfs files are read-only but `/etc/` is pre-configured.

## Dockerfile Changes

```dockerfile
FROM alpine:3.21 AS alpine_rootfs
RUN apk add --no-cache busybox
# Extract the complete rootfs

FROM scratch
# ... existing Kevlar binaries ...
COPY --from=alpine_rootfs / /alpine/
# Merge Alpine rootfs into the initramfs root
```

## Verification

```
make test-m10-phase1
# Expect: "alpine login:" prompt on serial output
```

Success: BusyBox init processes inittab, mounts proc/sys, spawns getty.
