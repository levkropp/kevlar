# Blog 131: Clock parity with Linux — 13/13 contract tests, nanosecond timestamps

**Date:** 2026-03-30
**Milestone:** M10 Alpine Linux — Phase 4 (Clock & Timestamp Parity)

## Summary

Achieved full clock behavior parity with Linux across 13 contract tests.
Fixed tmpfs timestamps (were always epoch 0), added nanosecond precision
to the VFS clock, and validated that file creation, write, utimensat, and
directory operations all produce correct timestamps matching wall clock.

## Clock contract test suite

New `testing/test_clock.c` with 13 tests covering:

| Category | Tests |
|----------|-------|
| clock_gettime | REALTIME plausible, REALTIME nsec, MONOTONIC increases |
| Clock agreement | REALTIME vs gettimeofday, gettimeofday usec precision |
| File timestamps | New file mtime, stat nsec, write updates mtime |
| utimensat | UTIME_NOW sets current time |
| clock_getres | Reports valid resolution |
| Additional clocks | MONOTONIC_RAW, BOOTTIME |
| Directory ops | mkdir updates parent mtime |

Results: **Linux 13/13, Kevlar 13/13** — zero delta.

## What was broken

### tmpfs timestamps were always epoch 0

The `File` and `Dir` structs in `kevlar_tmpfs` used `Stat::zeroed()` which
set all timestamps to 0 (January 1, 1970). No operation ever updated them:

- `File::new()` → mtime = 0
- `File::write()` → mtime unchanged
- `Dir::create_file()` → parent mtime unchanged
- `set_times()` → not implemented (VFS default = silent no-op)

This caused OpenRC's clock skew detection to loop: deptree on tmpfs had
mtime=0, which was always "older" than config files on ext4.

### VFS clock had only second-level granularity

`vfs_clock_secs()` returned `u32` seconds from epoch. The timer IRQ
updated it every 10ms (100 Hz) but only stored the second component.
`stat().st_mtim.tv_nsec` was always 0 for tmpfs-created files.

Linux reports nanosecond precision on tmpfs via `current_time()` which
reads the kernel's high-resolution timekeeping (TSC-based on x86).

## Fixes applied

### tmpfs timestamp support

Added `AtomicU32` timestamp fields to both `File` and `Dir`:

- **Creation:** `File::new()` and `Dir::new()` set atime/mtime/ctime to
  current wall clock via `vfs_clock_ts()`
- **Write:** `File::write()` updates mtime/ctime + mtime_nsec on every
  successful write
- **set_times:** Implemented for both `File` and `Dir` — enables
  `utimensat(UTIME_NOW)` and explicit timestamp setting
- **Directory ops:** `create_file`, `create_dir`, `create_symlink`,
  `unlink`, `rmdir` all call `self.touch()` to update parent dir mtime

### Nanosecond VFS clock

Added `vfs_clock_ts() -> (u32, u32)` returning (seconds, nanoseconds).
The kernel timer IRQ now calls `set_vfs_clock_ns(secs, nsec)` using the
full nanosecond wall clock from TSC:

```
wall_ns = read_wall_clock().nanosecs_from_epoch()
secs = wall_ns / 1_000_000_000
nsec = wall_ns % 1_000_000_000
```

The TSC provides ~1ns resolution calibrated via PIT at boot. Timer IRQ
updates the VFS clock at 100 Hz, so nsec values change every 10ms.

### stat() reports nanosecond timestamps

`File::stat()` now populates `atime_nsec`, `mtime_nsec`, `ctime_nsec`
from the stored `mtime_nsec` field. On Linux, `struct stat` reports these
via `st_atim.tv_nsec` / `st_mtim.tv_nsec` / `st_ctim.tv_nsec`.

## Clock infrastructure summary

| Component | Source | Precision | Notes |
|-----------|--------|-----------|-------|
| CMOS RTC | I/O ports 0x70/0x71 | 1 second | Read once at boot |
| TSC | rdtsc instruction | ~1 nanosecond | Calibrated via PIT |
| Wall clock | RTC epoch + TSC delta | nanosecond | `read_wall_clock()` |
| VFS clock | Timer IRQ (100 Hz) | 10ms updates, ns stored | `vfs_clock_ts()` |
| vDSO | Per-process mapped page | nanosecond | `__vdso_clock_gettime` |
| Monotonic | TSC-based | nanosecond | `read_monotonic_clock()` |

## Remaining clock gaps vs Linux

- **settimeofday/clock_settime:** Silent no-op stubs (return 0 without effect)
- **CLOCK_PROCESS_CPUTIME_ID:** Falls through to monotonic (no per-process accounting)
- **Leap seconds:** Not handled (matches most Linux configurations)
- **adjtimex/ntp:** Not implemented (no clock discipline)

## Files changed

- `services/kevlar_tmpfs/src/lib.rs` — File/Dir timestamps, set_times, touch on dir ops
- `libs/kevlar_vfs/src/lib.rs` — `vfs_clock_ts()`, `set_vfs_clock_ns()`
- `kernel/timer.rs` — Update VFS clock with nanosecond precision
- `testing/test_clock.c` — 13 clock contract tests
- `tools/build-initramfs.py` — Add test-clock to initramfs
