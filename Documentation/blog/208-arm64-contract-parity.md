## Blog 208: ARM64 → 159/159 contract parity in five bug fixes

**Date:** 2026-04-22

Blog 207 closed with 58/159 arm64 contract tests passing, 101
failing, and the honest admission that the arm64 backend had gone
un-owned for a month while x86_64 absorbed Milestone T and the
task-25 investigation.  This post covers the five bug fixes that
took arm64 from "builds and boots" to 100% contract parity with
x86_64.  All five bugs were dormant consequences of the arm64
backend getting less maintenance attention than the x86_64 backend —
none were architectural impossibilities, none required new research.

```
 58/159  (before any fixes)
 138/159 (SYS_SHM* constants)            +80
 140/159 (RTC + monotonic + vfork EINVAL) +2
 158/159 (wait_queue kernel-VA guard)    +18
 159/159 (vfork child SP)                +1
```

## Bug 1 — Buddy bitmap only covered 1 GiB, arm64 RAM starts at 1 GiB

From the previous post but worth repeating for narrative continuity.

`libs/kevlar_utils/buddy_alloc.rs` indexes its global allocation
bitmap by absolute `paddr / PAGE_SIZE`, capped at 262 144 entries
(1 GiB).  QEMU virt places arm64 RAM at 0x40000000 (1 GiB boundary)
and extends upward, so every page of RAM sits *past* the bitmap's
coverage.  The bitmap helpers silently early-return for out-of-range
indices, but the early-return is asymmetric: `alloc` treats out of
range as "not allocated yet" (pass the assert, set nothing),
whereas `free` treats out of range as "already freed" (assert
fails).  Result: the first `free_pages` of any page in managed arm64
RAM panics "buddy_alloc: double free page 0x42NNN000".

Fix: bump the bitmap to 4 M pages (16 GiB coverage, 512 KB of
static storage), which is fine for any real Kevlar host and leaves
headroom for future arches with even higher RAM bases.

**Contract impact:** every test that did a munmap — which is
*most* of them — panicked the kernel.  Fixing this took pass count
from 18/63 (sampled) to 58/159 (full suite), which tells you how
many tests got masked.

## Bug 2 — Six SYS_\* constants silently became variable bindings

`do_dispatch` in `kernel/syscalls/mod.rs` is a big `match n { ... }`
over ~200 syscall numbers.  Six identifiers — `SYS_SHMGET`,
`SYS_SHMAT`, `SYS_SHMCTL`, `SYS_SHMDT`, `SYS_SETRLIMIT`,
`SYS_GETRLIMIT` — were not defined in the arm64 `syscall_numbers`
module.

In Rust, a match pattern that looks like an uppercase identifier
resolves to a constant *if one is in scope*.  If nothing is in
scope, the identifier becomes a fresh *variable binding* — a
catch-all pattern that matches every value and binds it to the new
variable.  No error.

Because `SYS_SHMGET` was listed first among the unbound six (line
1416), *every syscall number from 101 onward* silently routed to
`sys_shmget` with wrong args, which rejected them with `-EINVAL`.
nanosleep, setaffinity, getrlimit, dozens more — all returned
EINVAL for no apparent reason.

The compiler *did* warn about this, twice per identifier:

```
warning: unused variable: `SYS_SHMGET`
warning: variable `SYS_SHMGET` should have a snake case name
```

But they were lost in the 342 warnings the ARM64 build already
emitted.  Two prompts I'll remember: *grep your build output for
"should have a snake case name" on every arm64 build before trusting
dispatch*, and *any arm64 syscall that returns a baffling EINVAL is
this bug until proven otherwise*.

Fix: define SYS_SHM* with their proper asm-generic/unistd.h numbers
(194-197); define SYS_SETRLIMIT / SYS_GETRLIMIT as 0xF010/0xF011
dummy slots since aarch64 uses prlimit64 (261) instead.

**Contract impact:** 58/159 → 138/159.  Eighty tests unstuck from a
single commit.

## Bug 3 — No RTC, no CNTPCT-based monotonic clock

Two related time-layer misses.  `arch::read_rtc_epoch_secs` was
stubbed to 0 on arm64 (`// ARM64 has no CMOS; returns 0`), so
`CLOCK_REALTIME` always returned `tv_sec = 0` — failing the
"plausible (>= 2023-11-14)" check.  And `MonotonicClock::nanosecs`
fell back to `ticks * 1_000_000_000 / TICK_HZ`, so any reader inside
the first 20 ms window saw `0`, failing `/proc/uptime`'s `up1 > 0`
check.

QEMU virt exposes PL031 at paddr 0x09010000 — a simple MMIO u32 at
offset 0 returns seconds since epoch, seeded from the host clock
at VM start.  boot.S already maps low paddrs as Device memory, so
the kernel straight-map read just works.  For the monotonic clock,
CNTPCT_EL0 + CNTFRQ_EL0 give precise ns-since-boot; routed it into
`MonotonicClock::ns_snapshot` so the existing x86_64 gate picks it
up on arm64 too.

**Contract impact:** 138/159 → 140/159.  +clock_realtime, +proc_global.

## Bug 4 — WaitQueue::wake_all's heap-corruption guard rejected every arm64 Arc

This one was the most instructive.

`wake_all` has a paranoia check: if a `Arc<Process>` popped from
the queue has a raw pointer below `0xffff_8000_0000_0000`, treat it
as corrupt, `mem::forget` it, and move on.  The constant is x86_64's
high-half boundary (canonical VAs have bit 47 set, so the kernel
high half starts at 0xffff_8000_0000_0000).

ARM64's canonical kernel VA base is **0xffff_0000_0000_0000** — the
high half starts at bit 48, not 47.  Every valid arm64 kernel Arc
pointer falls in `[0xffff_0000_0000_0000, 0xffff_8000_0000_0000)`
and was consequently forgotten by the guard.

Effect: every wait-queue wakeup was silently dropped on arm64.
`wait4`, `poll`, `pselect`, `accept`, `timerfd`, `epoll`, `futex` —
any blocking syscall that relied on wake_all — slept until the 20 s
contract-test timeout.  Fifteen tests timed out.  In serial
output, a single "WAITQ CORRUPT: bad arc=..." line appeared per
batch (the guard dedup'd its warn), and with every wake rejected
uniformly it looked like background noise rather than a smoking gun.

The debugging path was: trace SWITCH events → see child exit,
parent never runs again → trace sleep/wake at the WaitQueue layer →
see wake_all called with waiter_count=1 but `RESUME` never printed
→ read wake_all source → find the hardcoded 0xffff_8000.  Would
have been faster to spot with `ag 0xffff_8000 | grep -v x64`
earlier — I should have audited for arch-hardcoded kernel-VA
checks the moment arm64 started timing out on any blocking syscall.

Fix: replace the literal with `kevlar_platform::arch::KERNEL_BASE_ADDR`,
so both arches use their own high-half boundary.

**Contract impact:** 140/159 → 158/159.  Closed every timeout except
one.

## Bug 5 — vfork child inherits userspace SP

Remaining 1/159 after bug 4 was `process/vfork_basic`.  Child
(pid=2) faulted at `0xfffffffffffffff0` on its first stack write,
killed by SIGSEGV, test recorded status=0xb.

Cause: userspace `vfork()` calls
`clone(CLONE_VM | CLONE_VFORK | SIGCHLD, child_stack=0)` — child
runs on the parent's stack until exec/_exit, so it passes 0 as the
new-stack pointer.  Kevlar's `arch::new_thread` was passing that 0
through verbatim into the child's saved `sp_el0`, so the child
started with SP=0.  First push → fault at 0-16 = 0xfffffffffffffff0.

Fix: at the top of `new_thread`, if `child_stack == 0`, use
`frame.sp` (parent's current userspace SP) instead.  Three lines.

**Contract impact:** 158/159 → 159/159.  Full parity.

## What was actually wrong

All five bugs share a pattern: a piece of code *assumed* x86_64's
memory layout, constant set, or hardware topology, without a
matching arm64 story.  None of them were "arm64 is fundamentally
different from x86_64" — they were "this code was written for
x86_64, the author either forgot or didn't need to think about
arm64, and the arm64 backend didn't exist or wasn't tested against
at the time."

The lesson for the arm64 port isn't "be careful with assembly" —
arm64 assembly has been fine the whole time.  The lesson is: *any
unexamined numeric or constant in cross-arch code is a potential
arm64 bug.*  Bitmap sizes, VA boundaries, match patterns that look
like constants but aren't — these are the failure modes.

## What's next

With contract parity achieved, the remaining arm64 work splits
cleanly into two tracks:

1. **Benchmarks vs Linux ARM64 KVM** — does Kevlar arm64 keep up
   with Linux under HVF on Apple Silicon?  Any outsized regressions
   on syscall-heavy workloads?  The handoff prompt's M6.6 target
   was "27/28 within 10% of Linux" on x86_64 — matching that on
   arm64 is the next milestone.  Separate blog post.

2. **Feature gaps**: NMI watchdog (GICv3 FIQ), if-trace (DAIF
   tracking), ghost-fork CoW, proper ARM64 saved-context
   introspection.  These are tracked as known stubs in blog 207 —
   contract tests don't exercise them, but they're load-bearing for
   debugging and long-running workloads.

## Stats

- 5 commits
- 5 bugs
- +101 tests closed
- Full ARM64 contract parity with x86_64
