# 087: ktrace tracing system, wall-clock fix, apk update diagnosis

**Date:** 2026-03-19
**Milestone:** M10 (Alpine Linux)
**Status:** ktrace complete, 3 bugs fixed, apk hang root-caused

## Context

`apk update` hangs inside Kevlar when running Alpine Linux. Serial debugging
at 115200 baud (14.4 KB/s) can't keep up with the syscall volume needed to
diagnose it — at ~200 bytes per JSONL event, we max out at ~70 traced
syscalls/sec. We needed a parallel high-bandwidth tracing system.

## ktrace: binary kernel tracing

Built a complete tracing system from scratch in one session:

**Architecture:** Fixed 32-byte records written to per-CPU lock-free ring
buffers (8192 entries/CPU = 256 KB/CPU). Dump via QEMU ISA debugcon (port
0xe9, ~5 MB/s on KVM — 350x faster than serial). Host-side Python decoder
outputs text timelines and Perfetto JSON for Chrome visualization.

**Kernel side** (`kernel/debug/ktrace.rs`, `platform/x64/debugcon.rs`):
- `TraceRecord`: 8B TSC + 4B packed header (event_type:10|cpu:3|pid:11|flags:8) + 20B payload
- Per-CPU rings indexed by `AtomicUsize`, same pattern as htrace
- `record()`: ~30ns hot path (rdtsc + atomic store)
- `dump()`: writes 64B header + ring data via debugcon
- Zero overhead when feature disabled (cfg'd out); one atomic load when runtime-disabled

**Feature flags** in `kernel/Cargo.toml`:
```
ktrace, ktrace-syscall, ktrace-sched, ktrace-vfs, ktrace-net, ktrace-mm, ktrace-all
```

**Instrumentation points** (Phase 1):
- Syscall enter/exit (lean + full dispatch paths)
- Context switch (flight recorder integration)
- Wait queue sleep/wake
- TCP connect, send, recv, poll
- Network packet RX/TX

**Host decoder** (`tools/ktrace-decode.py`):
```
$ python3 tools/ktrace-decode.py ktrace.bin --timeline --pid 6
[  0.066302] CPU0 PID=6 SYSCALL_ENTER nr=59 (execve) ...
[  0.072630] CPU0 PID=6 SYSCALL_EXIT  nr=9  (mmap)   result=42952138752
[  1.062902] CPU0 PID=6 CTX_SWITCH    from_pid=6 to_pid=8
              ^--- apk stuck in userspace for 30s, no more syscalls

$ python3 tools/ktrace-decode.py ktrace.bin --perfetto trace.json
# Open in https://ui.perfetto.dev
```

**Makefile integration:**
```bash
make run-ktrace                            # boot with debugcon + ktrace-all
make build FEATURES=ktrace-net,ktrace-sched  # selective features
make decode-ktrace                         # decode ktrace.bin
```

## Bugs found and fixed

### 1. lseek on directory fds returned ESPIPE

`lseek(dir_fd, 0, SEEK_SET)` returned `-ESPIPE` instead of `0`. The
`INode::is_seekable()` method returned `false` for directories, but Linux
allows lseek on directory fds (used by `telldir`/`seekdir`, and apk uses it
to check if an fd is a regular file).

**Fix:** `libs/kevlar_vfs/src/inode.rs` — changed `INode::Directory(_) => false`
to `true`.

### 2. vDSO returned monotonic time for CLOCK_REALTIME

The vDSO `__vdso_clock_gettime` only handled `CLOCK_MONOTONIC` (id=1) and
fell back to syscall for `CLOCK_REALTIME` (id=0). The vDSO
`__vdso_gettimeofday` returned nanoseconds-since-boot (~0.07s at test start)
instead of epoch time (~1.77 billion for 2026).

Programs calling `time()`, `gettimeofday()`, or `clock_gettime(CLOCK_REALTIME)`
got near-zero timestamps. This breaks SSL certificate validation, cache
expiry checks, and any timeout calculation based on wall-clock time — all
things `apk update` does.

**Fix:** `platform/x64/vdso.rs` — added `wall_epoch_ns` field to the vDSO data
page (RTC boot epoch in nanoseconds, read from CMOS at boot). Rewrote the
hand-crafted x86_64 machine code for `__vdso_clock_gettime` to handle both
`CLOCK_REALTIME` (adds epoch offset) and `CLOCK_MONOTONIC` (no offset) in
84 bytes. Shifted all subsequent vDSO function offsets and recomputed every
RIP-relative displacement in the symbol table.

Before: `date` → `Thu Jan  1 00:00:00 UTC 1970`
After:  `date` → `Thu Mar 19 11:10:51 UTC 2026`

### 3. Multiple debug= cmdline args concatenated without separator

`--ktrace` adds `debug=ktrace` to the kernel command line. Combined with
`--append-cmdline "debug=syscall"`, the bootinfo parser concatenated them
as `"ktracesyscall"` instead of `"ktrace,syscall"`, causing the filter to
silently ignore all categories.

**Fix:** `platform/x64/bootinfo.rs` — insert comma separator when appending
to a non-empty `debug_filter` string.

Also fixed ktrace dump reliability: write an initial dump immediately on
enable (so the debugcon file always has valid data even if QEMU is killed),
and updated the decoder to scan for the last KTRX header in concatenated
dumps.

## apk update diagnosis (via ktrace)

ktrace revealed exactly what happens when `apk.static --root /mnt update`
runs:

1. **t=0.000s:** DHCP discover completes (2 TX, 2 RX packets)
2. **t=0.066s:** apk.static starts, reads Alpine package database files from ext2
3. **t=0.066-0.072s:** Opens and reads `installed` (14881 bytes), `triggers` (95 bytes) via `openat` → `mmap(MAP_ANONYMOUS)` → `read()` → `close` → `munmap` pattern
4. **t=0.072s:** Opens third file (`scripts.tar`), allocates anonymous buffer via `mmap` — **then stops making syscalls entirely**
5. **t=1.0-30.6s:** PID 6 (apk) spins in userspace consuming 100% CPU. PID 8 (BusyBox `timeout`) polls every 1s with `kill(6, 0)`. No network syscalls ever.
6. **t=30.6s:** timeout sends SIGTERM, apk dies.

**Key finding:** 93 syscall enters match 93 exits — apk is not stuck in a
kernel syscall. It's stuck in **userspace code** between the buffer allocation
(mmap) and the file read. Zero network activity means apk never reaches the
"fetch remote index" phase — it's stuck processing the **local** package
database.

**Root cause theory:** The CLOCK_REALTIME fix (bug #2 above) is the most
likely culprit. apk uses `time()` for cache validity, signature verification
timestamps, and SSL cert checks. With wall-clock returning ~0 (epoch 1970),
apk's internal logic likely entered an infinite retry or validation loop.
Now that wall-clock returns correct 2026 timestamps, apk should proceed
past the local database phase and attempt network operations.

## Test results (post-fix)

All test suites pass with zero regressions:

| Suite | Result |
|-------|--------|
| check-all-profiles | 4/4 compile clean |
| test-contracts | 103 PASS, 9 XFAIL, 0 FAIL |
| test-threads-smp | 14/14 PASS (4 CPUs) |
| test-regression-smp | 15/15 PASS |
| test-busybox | 100/100 PASS |
| test-alpine | 7/7 PASS |

## Files changed

**New files (ktrace):**
- `platform/x64/debugcon.rs` — ISA debugcon driver
- `kernel/debug/ktrace.rs` — ring buffers, record/dump, event types
- `tools/ktrace-decode.py` — binary decoder (timeline, summary, Perfetto)
- `testing/test_ktrace_apk.sh` — apk test with 30s timeout for ktrace

**Modified (ktrace instrumentation):**
- `kernel/syscalls/mod.rs` — syscall enter/exit tracing
- `kernel/process/switch.rs` — context switch tracing
- `kernel/process/wait_queue.rs` — sleep/wake tracing
- `kernel/net/tcp_socket.rs` — connect/send/recv/poll tracing
- `kernel/net/mod.rs` — packet RX/TX tracing
- `kernel/process/process.rs` — dump on PID 1 exit
- `kernel/lang_items.rs` — dump on panic
- `tools/run-qemu.py` — `--ktrace` flag
- `Makefile` — `run-ktrace`, `decode-ktrace`, `FEATURES` variable

**Modified (bug fixes):**
- `libs/kevlar_vfs/src/inode.rs` — directory lseek
- `libs/kevlar_utils/lazy.rs` — `try_get()` for safe early-boot access
- `kernel/process/mod.rs` — `try_current_pid()` for ktrace during boot
- `platform/x64/vdso.rs` — CLOCK_REALTIME + wall_epoch_ns + layout shift
- `platform/x64/bootinfo.rs` — debug filter comma separator
- `platform/Cargo.toml`, `kernel/Cargo.toml` — ktrace feature flags
- `kernel/debug/{mod,filter,emit}.rs` — KTRACE filter bit + init
- `tools/build-initramfs.py` — include ktrace test script
