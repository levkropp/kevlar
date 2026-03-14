# Milestone 9: systemd Init System

**Goal:** Boot a full systemd-based system. systemd manages services,
mounts filesystems, handles system shutdown, and provides essential system
services.

**Current state:** M8 provides cgroups v2 and namespaces. systemd binary exists
in the distro and can be statically linked or dynamically linked against musl.

**Impact:** With M9, Kevlar becomes a functional OS replacement rather than
just a kernel. Services like networking, logging, storage, and users can be
managed by systemd.

## Phases (revised after M8 completion)

| Phase | Name | Duration | Prerequisite |
|-------|------|----------|--------------|
| [1](phase1-syscall-gaps.md) | Syscall Gap Closure | 3-4 days | M8 |
| [2](phase2-init-sequence.md) | Init Sequence Validation | 4-5 days | Phase 1 |
| [3.1](phase3.1-build-systemd.md) | Build systemd Binary | 2-3 days | Phase 2 |
| [3.2](phase3.2-first-boot.md) | First Boot (sysinit.target) | 3-4 days | Phase 3.1 |
| [3.3](phase3.3-basic-target.md) | Service Startup (basic.target) | 3-4 days | Phase 3.2 |
| [4](phase4-services.md) | Service Management | 3-5 days | Phase 3.3 |

## Scope

- Kernel: Full M8 (cgroups v2, namespaces) + necessary syscalls for systemd
- Userspace: systemd + core system tools + musl libc
- Filesystem: Complete /proc, /sys, /dev, with mount namespace support
- Boot: systemd-init (PID 1) starts, manages services, reaches multi-user.target

## Challenges

1. **systemd is complex:** ~1 million lines of code. Requires D-Bus (IPC),
   journald (logging), udevd (device management), PAM (authentication), etc.
   We'll start with core systemd-init and expand.

2. **D-Bus dependency:** systemd uses D-Bus for inter-process communication.
   Need to run dbus-daemon or use libdbus in systemd. Consider using musl-built
   systemd to avoid glibc complexity.

3. **Device management:** udev/systemd-udevd requires /sys with proper device
   information. May need custom devtmpfs or mdev wrapper.

4. **Kernel interfaces:** systemd expects various kernel interfaces:
   - netlink for event notification
   - uevent for device events
   - seccomp for process sandboxing
   - apparmor/selinux for MAC (can skip initially)

5. **Service dependencies:** systemd units have complex ordering and dependency
   graph. Ensure all dependencies are available or masked.

## Test Plan

**Boot sequence:**
```
1. QEMU starts kernel
2. kernel starts systemd (PID 1)
3. systemd mounts /proc, /sys, /dev, /tmp, etc.
4. systemd starts basic services (getty, login)
5. Shell prompt appears
```

**Success criteria:**
- systemd prints boot output ("Reached multi-user.target")
- `systemctl` command works
- `systemctl list-units --type=service` shows active services
- Interactive shell login works
- `ps aux` shows systemd + services running
- `journalctl` shows boot log

## Integration Points

- **Kernel syscalls:** Ensure all systemd syscalls are implemented
- **Device nodes:** Create /dev/null, /dev/zero, /dev/random, /dev/pts/*, etc.
- **/run filesystem:** tmpfs mount for systemd runtime data
- **User/group database:** /etc/passwd, /etc/group (stub or real)
- **Service files:** /etc/systemd/system/*.service (bundled in initramfs)

## Known Issues

1. **glibc binaries:** If we use glibc-built systemd, requires full glibc compat
   from M7. Using musl-compiled systemd is simpler.

2. **Licensing:** systemd is LGPLv2+. Kevlar is MIT/Apache/BSD; ensure license
   compatibility if distributing together.

3. **Security:** Kevlar's Fortress profile will disable some systemd hardening
   (seccomp, apparmor). This is acceptable for initial boot.

## Future Work

After M9, Kubuntu desktop is still far away (X11/Wayland/GPU required).
But a headless Kevlar system running systemd services is achievable and valuable.

Possible next steps:
- **M10a: Container runtime** (crun/runc) for running OCI containers
- **M10b: Distro packaging** (building .deb packages on Kevlar)
- **M10c: GUI stack** (X11, Wayland, desktop environment) — very long term
