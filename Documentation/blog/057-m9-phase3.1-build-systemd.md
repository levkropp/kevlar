# M9 Phase 3.1: Build systemd Binary

Phase 3.1 adds Ubuntu 20.04's prebuilt systemd v245 binary to Kevlar's
initramfs, along with all glibc runtime dependencies.

## Approach: prebuilt binaries

Rather than compiling systemd from source, we extract the Ubuntu 20.04
package's prebuilt binaries. This aligns with Kevlar's goal of being a
drop-in Linux kernel replacement — if we can't run unmodified distro
binaries, we can't run prebuilt GPU drivers either.

The Dockerfile `apt-get install systemd`, then extracts the binaries
and their complete glibc dependency tree via `ldd`.

## What's in the initramfs

- `/usr/lib/systemd/systemd` — PID 1 binary (dynamically linked)
- `/usr/lib/systemd/systemd-journald` — logging daemon
- `/bin/systemctl` — service control tool
- `/lib/x86_64-linux-gnu/` — 30+ glibc shared libraries
- `/lib64/ld-linux-x86-64.so.2` — dynamic linker
- `/etc/systemd/system/default.target` — boot target
- `/etc/systemd/system/kevlar-getty.service` — console shell
- `/etc/machine-id`, `/etc/os-release`, `/etc/fstab`

## First boot result

systemd starts, glibc initializes, the dynamic linker resolves all
libraries — then systemd exits with status 1 (configuration error).
This is expected: it can't find the mount points and configuration
it needs. Phase 3.2 will fix these iteratively.

The critical milestone: **an unmodified distro binary executes on
Kevlar through the full glibc init sequence.**
