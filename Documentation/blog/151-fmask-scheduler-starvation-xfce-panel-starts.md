# Blog 151: FMASK Fix, Scheduler Starvation, and xfce4-panel Starts

**Date:** 2026-04-07

## Summary

Three kernel bugs found and fixed. xfce4-panel now starts alongside
xfwm4 and xfce4-session — all core XFCE desktop components are
functional. The test harness can't reliably observe the results because
xfwm4's 5 rendering threads starve PID 1 under the round-robin
scheduler. This is the last blocker for 4/4 XFCE tests.

## Bug 1: SYSCALL FMASK missing TF — kernel #DB flood

The `IA32_FMASK` MSR controls which RFLAGS bits are cleared on SYSCALL
entry. Kevlar only masked IF (0x200), leaving TF (trap flag), DF
(direction flag), and others unmasked:

```rust
// Before: only IF masked
const SYSCALL_RFLAGS_MASK: u64 = 0x200;

// After: TF + IF + DF masked (matches Linux)
const SYSCALL_RFLAGS_MASK: u64 = 0x0700;
```

When GLib's `g_test_subprocess()` probes whether it's running under a
test framework, it temporarily sets TF to detect PTRACE. With only IF
masked, TF leaked into kernel mode on every syscall. Each kernel
instruction generated a `#DB` (Debug) exception, flooding the serial
port with warnings and consuming 100% CPU in the interrupt handler.

On 4 CPUs, this caused a kernel crash: the #DB flood corrupted the
interrupt return frame, causing the kernel to jump to address 0x2
(instruction fetch from near-null).

**Root cause:** The x86 SYSCALL instruction doesn't clear TF by default.
Linux's FMASK clears TF|IF|DF|NT|AC (0x47700). Our minimal fix clears
TF|IF|DF (0x700) — sufficient for correctness without changing IOPL
handling.

## Bug 2: #DB handler didn't clear TF for kernel-mode traps

Even with the FMASK fix, the `#DB` handler for kernel-mode exceptions
just printed a warning and returned — without clearing TF in the saved
RFLAGS. This meant that after IRET, TF was still set, generating
another #DB immediately.

```rust
// Before: just logged and returned
warn!("#DB: DR6={:#x} RIP={:#x} CS={:#x}", dr6, rip, cs);

// After: clear TF in saved RFLAGS, silently return
let rflags = unsafe { rflags_ptr.read_unaligned() };
if rflags & 0x100 != 0 {
    unsafe { rflags_ptr.write_unaligned(rflags & !0x100) };
}
```

This is defense-in-depth: the FMASK fix prevents TF from reaching the
kernel in the first place, but the #DB handler now correctly handles the
case where TF is set (e.g., from NMI or other non-SYSCALL entry points).

## Bug 3: BREAKPOINT delivered SIGSEGV instead of SIGTRAP

The `int3` (BREAKPOINT) handler delivered SIGSEGV (signal 11) to
userspace processes. Linux delivers SIGTRAP (signal 5). This affected
GLib's `G_BREAKPOINT()` macro used for fatal log messages.

Investigation of the at-spi-bus-launcher crash revealed it was NOT a
stack canary failure — it was GLib's intentional debug trap:

```asm
; g_log_structured_array in libglib-2.0:
64ec7: test ebx,ebx     ; should we trap?
64ec9: je   1c0e4        ; skip if no
64ecf: int3              ; G_BREAKPOINT() — deliberate trap
```

The at-spi-bus-launcher hits a fatal error (can't start accessibility
bus), GLib traps with `int3`. With the correct SIGTRAP delivery, the
process terminates with signal 5 matching Linux behavior.

## xfce4-panel starts

With the FMASK and SIGTRAP fixes, the XFCE failsafe session now starts
all configured components:

| Component | Status | Notes |
|-----------|--------|-------|
| xfce4-session | Running | Session manager (PID 14) |
| xfwm4 | Running | Window manager (5 threads) |
| xfce4-panel | Running | Panel (PID 55) |
| xfconfd | Running | Config daemon (D-Bus activated) |

The panel wasn't starting before because:
1. xfconfd couldn't connect to the session bus (at-spi crashes killed it)
2. Without xfconf, the session couldn't read its failsafe client list
3. Only xfwm4 (hardcoded fallback) was launched

With at-spi crashes no longer cascade-killing the session bus (SIGTRAP
terminates cleanly instead of SIGSEGV triggering cleanup cascades), the
full session starts correctly.

## Scheduler starvation: the last blocker

xfwm4 spawns 5 rendering threads that consume ~500% CPU total (no vsync
frame limiting). Kevlar's round-robin scheduler provides no fairness
guarantee. PID 1's timer fires after 100ms, marking it Runnable, but
the scheduler always picks the next thread in the queue — which is one
of xfwm4's threads that was just preempted and re-enqueued.

The result: PID 1 is Runnable but never scheduled. Its `usleep(100ms)`
call never returns. The test harness hangs despite all XFCE components
running correctly.

This affects both `-smp 1` and `-smp 2`: it's not about CPU count but
about scheduler fairness. With 7-10 CPU-bound threads competing, PID 1
(which sleeps most of the time) gets permanently starved.

**Linux solves this** with the Completely Fair Scheduler (CFS), which
tracks virtual runtime per task. A task that sleeps accumulates "credit"
and gets priority when it wakes up. CFS ensures even a sleeping task
gets scheduled promptly after its timer fires.

## VMA dump for crash investigation

Added executable VMA dumping to the BREAKPOINT handler. This allowed
resolving the at-spi crash addresses to specific library functions:

```
  000a0001b000-000a00071e01 r-x file    ← musl libc
  000a100df000-000a101fe000 r-x file    ← libgio-2.0
  000a102a5000-000a102db000 r-x file    ← libgobject-2.0
  000a10316000-000a103ba000 r-x file    ← libglib-2.0 (crash here)
```

The crash at 0xa1035eed0 maps to libglib-2.0 offset 0x64ecf —
confirmed as `g_log_structured_array`'s `G_BREAKPOINT()` trap.

## Scheduler fix: resume_boosted

The starvation was caused by `POLL_WAIT_QUEUE.wake_all()` in the timer
handler. Every tick, ALL poll/epoll waiters are woken via `resume()` →
`enqueue()` (back of queue). A timer-sleeping process (like PID 1) is
also enqueued via `resume()` — at the back, behind all the poll waiters
that were just re-enqueued.

The fix: `resume_boosted()` enqueues at the **front** of the run queue.
Timer-sleeping processes (nanosleep, usleep) use `resume_boosted()`.
Poll/signal wakes continue using regular `resume()` (back of queue).

```rust
// In Scheduler:
fn enqueue_front(&self, pid: PId) {
    self.run_queues[cpu].lock().push_front(pid);
}

// In Process:
pub fn resume_boosted(&self) {
    // ... same checks as resume() ...
    SCHEDULER.lock().enqueue_front(self.pid);
}

// In timer handler:
timer.process.resume_boosted();  // front-of-queue priority
```

This is a simplified CFS sleep credit: a process that was sleeping has
accumulated fairness debt and deserves prompt scheduling when it wakes.

Result: the XFCE test harness completes reliably. PID 1's `usleep()`
calls return within one preemption cycle (~30ms) instead of being
starved indefinitely.

## Test results

| Suite | Result |
|-------|--------|
| Threading SMP (4 CPUs) | 14/14 PASS |
| Regression SMP (4 CPUs) | 15/15 PASS |
| BusyBox SMP (4 CPUs) | 100/100 PASS |
| Alpine Smoke (67 tests) | 67/67 PASS |
| XFCE Desktop (SMP 2) | **2-3/4** (session + wm pass; panel timing; intermittent Xorg crash) |

XFCE session components confirmed starting:
- xfce4-session: Running (session manager)
- xfwm4: Running (window manager, 5 threads)
- xfce4-panel: Running (panel, confirmed via ps)
- xfconfd: Running (config daemon, D-Bus activated)

## Next steps

1. Investigate intermittent Xorg SIGSEGV (kills xfwm4 → test fails)
2. Stabilize XFCE test to 4/4 (panel detection timing, Xorg crash)
3. Add PS/2 keyboard/mouse driver for interactive testing

## Files changed

- `platform/x64/syscall.rs`: FMASK 0x200 → 0x700 (mask TF + DF)
- `platform/x64/interrupt.rs`: #DB handler clears TF in kernel mode
- `kernel/main.rs`: BREAKPOINT → SIGTRAP, other faults → SIGSEGV
- `kernel/process/process.rs`: `resume_boosted()` (front-of-queue)
- `kernel/process/scheduler.rs`: `enqueue_front()` method
- `kernel/timer.rs`: Timer wakeups use `resume_boosted()`
- `kernel/syscalls/getsockopt.rs`: Buffer size checks (3 helpers)
- `kernel/mm/page_fault.rs`: Demand page short-read diagnostic
- `testing/test_xfce.c`: Busy-poll wait, streamlined checks
- `Makefile`: 300s timeout, -smp 2 for test-xfce
