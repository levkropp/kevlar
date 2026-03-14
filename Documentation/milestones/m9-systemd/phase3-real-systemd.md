# Phase 3: Real systemd Boot

**Duration:** 5-7 days (iterative)
**Prerequisite:** Phase 2 (mini_systemd_v3 passes)
**Goal:** Boot systemd v245 (Ubuntu 20.04) as PID 1, reach basic.target.

## systemd Binary Build

Build systemd v245 with minimal config against musl (static) or glibc:

```
meson setup build \
  -Dstatic=true \
  -Dpam=false -Dselinux=false -Dapparmor=false -Daudit=false \
  -Dseccomp=false -Dutmp=false -Dgcrypt=false -Dp11kit=false \
  -Dgnutls=false -Dopenssl=false -Dcurl=false -Dlibfido2=false \
  -Dtpm2=false -Dzlib=false -Dbzip2=false -Dzstd=false -Dlz4=false \
  -Dxz=false -Dpolkit=false -Dblkid=false -Dkmod=false -Didn=false \
  -Dresolve=false -Dnetworkd=false -Dtimesyncd=false -Dlogind=false \
  -Dmachined=false -Dimportd=false -Dhomed=false -Dhostnamed=false \
  -Dlocaled=false -Dtimedated=false -Dcoredump=false -Dfirstboot=false \
  -Drandomseed=false -Dhwdb=false -Drfkill=false -Dhibernate=false \
  -Dportabled=false -Duserdb=false -Doomd=false
```

This yields systemd PID 1 + journald with minimal dependencies.

## Initramfs Contents

```
/usr/lib/systemd/systemd          — PID 1 binary
/usr/lib/systemd/systemd-journald — logging daemon
/etc/systemd/system/default.target → multi-user.target
/etc/systemd/system/kevlar-getty.service
/etc/passwd                        — root:x:0:0:root:/root:/bin/sh
/etc/group                         — root:x:0:
/etc/fstab                         — empty
/etc/machine-id                    — random 32-hex-char ID
```

## Expected Failure Points (in boot order)

| # | Issue | Fix |
|---|-------|-----|
| 1 | `personality(0xffffffff)` probe | Stub: return `PER_LINUX` (0) |
| 2 | `mount(NULL, "/", NULL, MS_REC\|MS_PRIVATE, NULL)` | Handle NULL source/fstype in mount.rs |
| 3 | `/dev/kmsg` write | Phase 2 adds this |
| 4 | `seccomp()` calls | Return ENOSYS (systemd handles gracefully) |
| 5 | `/proc/sys/kernel/random/boot_id` | Phase 2 adds this |
| 6 | `CLOCK_BOOTTIME` | Phase 2 adds this |
| 7 | AF_NETLINK socket | Return EAFNOSUPPORT (systemd skips udev) |
| 8 | `/proc/1/environ` | Phase 2 adds this |
| 9 | D-Bus socket connect | Fail gracefully (degraded mode) |
| 10 | `/proc/1/mountinfo` | Already implemented (M8) |

## Iterative Fix Cycle

1. Boot systemd in QEMU
2. Capture serial output + syscall trace
3. Identify first failure (unimplemented syscall or missing file)
4. Fix in kernel
5. Rebuild and retry
6. Repeat until basic.target reached

## Files to Create/Modify

- `testing/Dockerfile` — systemd v245 build stage (largest addition)
- `kernel/syscalls/personality.rs` — stub returning 0
- `kernel/syscalls/mount.rs` — handle NULL source/fstype edge cases
- Various /proc and /sys additions as discovered during fix cycle

## Success Criteria

- [ ] systemd PID 1 prints banner to serial
- [ ] systemd reaches sysinit.target (mounts, cgroups set up)
- [ ] systemd reaches basic.target
- [ ] journald starts and writes to /run/log/journal/
- [ ] Boot completes within 30 seconds in QEMU
