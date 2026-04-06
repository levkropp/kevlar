# Blog 147: XFCE Session Runs — Three Kernel Bugs Found

**Date:** 2026-04-06

## Summary

XFCE4 session manager now runs on Kevlar. The session starts xfce4-session,
xfconfd, xfsettingsd, and Xorg with the fbdev driver. Three kernel bugs were
discovered and fixed/documented during this work.

## Bug 1: Eager `release_stacks()` Use-After-Free

**Severity:** Critical (kernel crash)
**Status:** Fixed (workaround)

The `switch()` function in `kernel/process/switch.rs` eagerly freed exiting
processes' kernel stacks immediately after context-switching away from them.
On SMP, another CPU could pick the just-exited process from the scheduler
queue (due to a narrow re-enqueue window during process exit) and attempt to
context-switch to the freed stack, causing a NULL RIP page fault.

**Fix:** Disabled eager `release_stacks()`. Stacks are now freed lazily via
`gc_exited_processes()` from the idle thread, matching Linux's pre-4.9 behavior.
The tradeoff is ~32KB more memory per zombie until wait4() reaps them.

## Bug 2: `_xsave64` / `_xrstor64` Intrinsic Stack Corruption

**Severity:** Critical (kernel crash, single CPU)
**Status:** Documented, workaround applied

The Rust compiler's `_xsave64` and `_xrstor64` intrinsics (from
`core::arch::x86_64`) corrupt the kernel stack during context switches. The
exact cause is unknown — likely related to the `x86-softfloat` target spec
which disables SSE in compiler codegen but leaves SSE hardware enabled.

The corruption manifests as zeroed-out return addresses on the kernel stack,
causing RIP=0 page faults after `do_switch_thread`'s `ret` instruction.

**Workaround:** FPU state save/restore is disabled in `switch_task()`.
User-visible effect: SSE/AVX register contents leak between processes. This
is acceptable for now since most Alpine packages don't rely on SSE register
preservation across context switches (musl saves/restores in user space).

## Bug 3: SMP Deadlock During Concurrent Process Exits

**Severity:** High (system hang)
**Status:** Undiagnosed, workaround applied

When multiple XFCE processes crash simultaneously (e.g., at-spi-bus-launcher
hitting `__stack_chk_fail`), the system hangs with both CPUs stuck. Timer
interrupts stop firing on all CPUs. No panic message is produced, suggesting
the CPUs are spinning with interrupts disabled (spinlock deadlock).

The deadlock likely involves the process exit path (`Process::exit()`) which
acquires SCHEDULER, EXITED_PROCESSES, PROCESSES, and children locks.
Concurrent exits on two CPUs could create a lock ordering violation.

**Workaround:** XFCE test runs with `-smp 1`. The SMP deadlock does not
affect other tests (BusyBox, Alpine smoke, threading) because those don't
trigger concurrent process crashes.

## XFCE Component Status

| Component | Status | Notes |
|-----------|--------|-------|
| Xorg + fbdev | Running | 1024x768x32 framebuffer |
| xfce4-session | Running | Session manager with threads |
| xfconfd | Running | Configuration daemon |
| xfsettingsd | Running | Settings daemon |
| xfwm4 | Exits | "Xfconf could not be initialized" |
| xfce4-panel | Not starting | Depends on xfwm4/xfconf |
| at-spi-bus-launcher | Crashes | Stack canary failure (non-critical) |

## Next Steps

1. Fix xfconf initialization (D-Bus session setup / machine-id)
2. Investigate SMP deadlock in process exit path
3. Root-cause the xsave intrinsic corruption (examine compiler codegen)
4. Fix at-spi-bus-launcher stack canary failure
