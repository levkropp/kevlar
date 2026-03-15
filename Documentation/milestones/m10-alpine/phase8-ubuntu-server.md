# M10 Phase 8: Ubuntu Server Equivalence

**Goal:** Boot Ubuntu Server 24.04 (or 22.04) on Kevlar with systemd,
ext4 root, apt package management, and Docker-ready baseline.

## Why Ubuntu Server

Ubuntu Server is the most deployed Linux server distro. If Kevlar can
run it, it can run anything in the server space. This phase combines
everything from Phases 1-7 with Ubuntu-specific requirements.

## Scope

### Ubuntu Server rootfs

Build a minimal Ubuntu Server rootfs:
- `debootstrap --variant=minbase jammy /rootfs`
- Or use Ubuntu's cloud image (pre-built ext4 disk image)
- systemd (already proven in M9), apt, OpenSSH

### apt package management

apt needs:
- Writable ext4 root (Phase 4)
- DNS resolution (Phase 6)
- HTTPS (libssl/libcurl — glibc linked)
- dpkg file operations (create, rename, chmod, chown)
- `/var/lib/dpkg/` state database

### systemd on ext4 root

M9's systemd booted from initramfs. Now boot from ext4:
- GRUB or direct kernel boot with `root=/dev/vda1`
- systemd reads units from ext4 filesystem
- journald writes to `/var/log/journal/` on disk
- Multi-user.target with real services

### Docker-ready baseline

Docker needs:
- Namespaces: PID (done), mount (done), UTS (done), net (Phase 6), user
- cgroups v2 (done, needs enforcement in Phase 5)
- overlayfs or similar union filesystem
- seccomp (stub — return success)
- Network bridge / veth pairs

Full Docker is stretch but "docker-ready" means the kernel interfaces exist.

### Additional syscalls

Ubuntu Server userland (coreutils, systemd services, OpenSSH) may need:
- `fallocate()` — pre-allocate file space (ext4)
- `fadvise64()` — filesystem hints
- `splice()` / `tee()` — zero-copy pipe operations (we have stubs)
- `io_uring` — modern async I/O (can defer)
- `seccomp()` — sandbox syscall filtering (stub → allow all)
- `personality()` — process execution domain (stub → return 0)

## Verification

```
# Boot Ubuntu Server 24.04 on ext4 root
qemu ... -drive file=ubuntu-server.img,if=virtio
# Login as ubuntu user
apt update && apt install curl
curl https://example.com
systemctl status sshd  # running
docker run hello-world  # stretch goal
```

## M10 Complete

With Phase 8, Kevlar runs production-grade server workloads:
- Alpine Linux (musl + OpenRC) — fully operational
- Ubuntu Server (glibc + systemd) — ext4 root, apt, SSH
- Both use unmodified distro binaries — drop-in kernel replacement proven
