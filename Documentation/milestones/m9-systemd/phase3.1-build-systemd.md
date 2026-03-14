# Phase 3.1: Build systemd Binary

**Duration:** 2-3 days
**Prerequisite:** Phase 2 (mini_systemd_v3 passes 25/25)
**Goal:** Produce a statically-linked systemd v245 binary + journald in the initramfs. No kernel changes — this is pure build infrastructure.

## Build Approach

systemd v245 (Ubuntu 20.04 LTS) with minimal meson config. Build
against musl for static linking to avoid glibc NSS/dlopen complexity.

If musl build proves too difficult (systemd has glibc-isms), fall back
to glibc static build — we have glibc support from M7.

## Meson Config

Disable everything non-essential:

```
meson setup build \
  -Dstatic-libsystemd=true \
  -Dstandalone-binaries=true \
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

## Initramfs Filesystem Layout

```
/usr/lib/systemd/systemd              — PID 1 binary
/usr/lib/systemd/systemd-journald     — logging daemon
/etc/systemd/system/default.target    — symlink to multi-user.target
/etc/systemd/system/multi-user.target
/etc/systemd/system/basic.target
/etc/systemd/system/sysinit.target
/etc/systemd/system/local-fs.target
/etc/systemd/system/sockets.target
/etc/systemd/system/kevlar-getty.service
/etc/passwd                            — root:x:0:0:root:/root:/bin/sh
/etc/group                             — root:x:0:
/etc/fstab                             — empty
/etc/machine-id                        — 32-char hex ID
/etc/os-release                        — NAME="Kevlar"
/var/log/journal/                      — journal directory
/run/                                  — tmpfs mount point
```

## Unit Files

**default.target:**
```ini
[Unit]
Description=Default Target
Requires=basic.target
After=basic.target
AllowIsolate=yes
```

**kevlar-getty.service:**
```ini
[Unit]
Description=Kevlar Console Shell

[Service]
Type=idle
ExecStart=/bin/sh
StandardInput=tty
StandardOutput=tty
StandardError=tty
Restart=always

[Install]
WantedBy=multi-user.target
```

## Dockerfile Stage

New `systemd` build stage in `testing/Dockerfile`:
- Base: Ubuntu 20.04
- Install build deps: meson, ninja, gcc, pkg-config, libcap-dev, gperf
- Download systemd v245 tarball
- Configure with minimal flags
- Build systemd + systemd-journald
- Copy binaries + unit files to initramfs

## Files to Create/Modify

- `testing/Dockerfile` — add systemd build stage + COPY lines
- `testing/systemd/` — directory with unit files, machine-id, os-release
- `Makefile` — add `test-systemd-boot` target

## Success Criteria

- [ ] Docker build produces systemd + journald binaries
- [ ] Binaries are in the initramfs at correct paths
- [ ] Unit files and /etc layout are in place
- [ ] `make build` completes (even if systemd doesn't boot yet)
- [ ] All existing tests still pass (no regression)

## Fallback Plan

If systemd v245 proves too hard to build statically:
1. Try v244 or v243 (older = fewer deps)
2. Try dynamic build with glibc (we support it from M7)
3. Try systemd-stub (minimal init shim from systemd project)
4. Build elogind instead (lighter alternative)
