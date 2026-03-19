# Blog 090: Five test fixes — from red to full green across all suites

**Date:** 2026-03-19
**Milestone:** M10 Alpine Linux

## Context

After the nine-bug `apk update` fix session (blog 089), we had a working HTTP
fetch but several test suites still had failures. A systematic sweep through
every test target uncovered five distinct bugs spanning the futex subsystem,
UTS namespace caching, ext2 mount flags, and process lifecycle management.

## Bug 1: FUTEX_CLOCK_REALTIME not stripped from op mask

**Test:** glibc-threads — 0/14 (immediate crash: "The futex facility returned
an unexpected error code")

**Root cause:** glibc's NPTL calls `futex(addr, FUTEX_WAIT_BITSET | FUTEX_PRIVATE
| FUTEX_CLOCK_REALTIME, ...)` which encodes as op=0x109. Our CMD_MASK only
stripped `FUTEX_PRIVATE_FLAG` (0x80), not `FUTEX_CLOCK_REALTIME` (0x100):

```rust
const FUTEX_CMD_MASK: i32 = !(FUTEX_PRIVATE_FLAG);
// 0x109 & ~0x80 = 0x89 → no match → ENOSYS
```

glibc treats ENOSYS from futex as a fatal error and aborts before any test
runs.

**Fix:** Add FUTEX_CLOCK_REALTIME to the mask:

```rust
const FUTEX_CMD_MASK: i32 = !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);
// 0x109 & ~0x180 = 0x09 = FUTEX_WAIT_BITSET ✓
```

**Result:** glibc-threads 0/14 → **14/14**.

## Bug 2: sethostname doesn't invalidate cached utsname

**Test:** cgroups-ns `ns_uts_isolate` and `ns_uts_unshare` — 12/14

**Root cause:** The vDSO optimization (M9.9) added a per-process cached
utsname buffer for fast `uname(2)` dispatch. `sys_sethostname()` correctly
updated the UTS namespace object but never rebuilt the cache. Subsequent
`uname()` calls returned the stale pre-sethostname hostname.

The test sequence:
1. `unshare(CLONE_NEWUTS)` — create private UTS namespace ✓
2. `sethostname("child-host", 10)` — update namespace, but cache stale ✗
3. `uname(&u)` — reads cached buffer → still shows old hostname ✗

**Fix:** Call `proc.rebuild_cached_utsname()` after `set_hostname()` and
`set_domainname()` in the sethostname/setdomainname syscall handlers.

**Result:** cgroups-ns 12/14 → **14/14**.

## Bug 3: MS_RDONLY flag ignored in mount(2)

**Test:** ext2 `ext2_readonly` — 30/31

**Root cause:** The mount syscall defined constants for MS_NOSUID, MS_NODEV,
MS_NOEXEC, MS_REMOUNT, MS_BIND, MS_REC, and MS_PRIVATE — but not MS_RDONLY
(0x1). When `mount("none", "/tmp/mnt", "ext2", MS_RDONLY, NULL)` was called,
the read-only flag was silently ignored. Opening a file for writing on the
read-only ext2 mount succeeded instead of returning EROFS.

**Fix:** Three-layer enforcement:
1. Define `MS_RDONLY = 1` in the mount syscall handler
2. Add `readonly: bool` to `MountEntry` and `MountPoint`, with
   `mount_readonly()` and `MountTable::is_readonly(path)` helpers
3. Check `MountTable::is_readonly()` in `sys_open` and `sys_openat` before
   O_CREAT/O_WRONLY/O_RDWR operations, returning EROFS

**Result:** ext2 30/31 → **31/31**.

## Bug 4: vDSO page leaked on every fork

**Test:** busybox — 97–98/100 (GPF/SIGSEGV after ~130 forks)

**Root cause:** `alloc_process_page()` allocates a per-process vDSO data page
(4 KB) during fork. `Process::drop()` never freed it. After ~130 forks in the
busybox test suite, 520 KB of leaked pages put the page allocator under
pressure, causing it to return corrupted pages for subsequent process stacks.

**Fix:** Free the vDSO page in `Process::drop()`:

```rust
let vdso_paddr = self.vdso_data_paddr.load(Ordering::Relaxed);
if vdso_paddr != 0 {
    free_pages(PAddr::new(vdso_paddr as usize), 1);
}
```

## Bug 5: GC starvation under CPU-busy workloads

**Test:** busybox — still failing even with vDSO fix

**Root cause:** `gc_exited_processes()` only ran when the idle thread was
active (`current_process().is_idle()`). During the 100-test busybox suite, the
CPU was 100% busy — the idle thread never ran. Exited processes accumulated
in `EXITED_PROCESSES`, and their resources were never freed:

- Per process leaked: 1 vDSO page (4 KB) + 4 kernel stack pages (16 KB)
- After 130 processes: **2.5 MB** of kernel stacks + 520 KB vDSO pages
- Page allocator under pressure → stale/corrupted pages → GPF/SIGSEGV

The `is_idle()` guard was overly conservative. Exited processes have already
called `switch()` to yield the CPU, so their kernel stacks are not on any CPU
and are safe to free from any context.

**Fix:** Remove the `is_idle()` guard. GC now runs from any interrupt exit
path (timer IRQ, device IRQ), ensuring exited processes are reclaimed promptly
even under sustained CPU load.

**Result:** busybox 97/100 → **100/100**.

## Debugging approach

The futex bug was found by running with ktrace-syscall and checking the futex
return value: `-38` (ENOSYS) for op `0x109`. Decoding the op bits revealed the
missing FUTEX_CLOCK_REALTIME flag.

The UTS bug was found by tracing the data flow: `sethostname` → `ns.uts` →
(missing link) → `cached_utsname` → `uname()`. The cache was a vDSO
optimization that wasn't wired to the write path.

The ext2 bug was found by reading the test assertion: "expected EROFS, got
fd=4". Grepping for MS_RDONLY in the mount handler confirmed it was never
defined.

The resource leaks were the hardest — symptoms shifted with kernel binary
layout changes (classic Heisenbug). The key insight was that tests passed
individually (even 200 iterations) but failed in the full suite, and only
after ~130 processes. This pointed to accumulated resource exhaustion rather
than a logic bug in any individual syscall.

## Final test scorecard

| Suite | Before | After |
|---|---|---|
| BusyBox | 97/100 | **100/100** |
| BusyBox SMP | 100/100 | **100/100** |
| Contracts | 104/118 (0 FAIL) | **104/118 (0 FAIL)** |
| Cgroups/NS | 12/14 | **14/14** |
| ext2 | 30/31 | **31/31** |
| glibc threads | 0/14 | **14/14** |
| SMP threads | 14/14 | **14/14** |
| systemd v3 | 25/25 | **25/25** |
| KVM benchmarks | 42 faster, 0 reg | **42 faster, 0 reg** |
| apk update | exit 0 | **exit 0** |

## Files changed

- `kernel/syscalls/futex.rs` — FUTEX_CLOCK_REALTIME in CMD_MASK
- `kernel/syscalls/sethostname.rs` — rebuild_cached_utsname after set
- `kernel/process/process.rs` — rebuild_cached_utsname(), vDSO free, eager GC
- `kernel/fs/mount.rs` — MountEntry/MountPoint readonly flag, is_readonly()
- `kernel/syscalls/mount.rs` — MS_RDONLY definition and enforcement
- `kernel/syscalls/open.rs` — EROFS check for readonly mounts
- `kernel/syscalls/openat.rs` — EROFS check for readonly mounts
