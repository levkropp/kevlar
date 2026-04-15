# Blog 175: Pivot Back to Kevlar+XFCE — Baseline + 3 Quick Wins

**Date:** 2026-04-13

## Why pivot

Phase 12 of kxserver (blogs 173–174) achieved its real goal: a
working **diagnostic baseline**. We can render xterm, dmenu, and a
GTK3 window through our own Rust X server, so we know which X
protocol features XFCE actually relies on. The kxserver work was
not the destination — it was the lab to figure out what
"working X" looks like before going back to chase the real bugs in
**Xorg-running-on-Kevlar**, which is the actual goal: drop-in
Linux replacement booting Alpine + XFCE in QEMU.

This session pivots back to that.

## Run 1: capture the actual failure mode

```
$ timeout 360 make test-xfce PROFILE=balanced
…
TEST_PASS mount_rootfs
=== Phase 5: XFCE Session ===
  startxfce4 check: /usr/bin/startxfce4
  T+0 about to start_bg dbus-daemon
  T+0 dbus-daemon start_bg returned
  T+1 about to start_bg xfce4-session
  T+1 xfce4-session start_bg returned
PID 24 (dbus-daemon --session …) killed by signal 9
PID 31 (/usr/bin/iceauth source /tmp/.xfsm-ICE-S885N3) killed by signal 11
  T+2 sleeping
  …
PID 32 (xfwm4) killed by signal 11
  XFCE wait done
  components: wm=0 panel=0 session=0
TEST_FAIL xfwm4_running
TEST_FAIL xfce4_panel_running
TEST_FAIL xfce4_session_running
TEST_END 1/4
```

Two distinct user-mode crashes:

1. **iceauth (PID 31) — GP fault at indirect call site**
   ```
   ip=0xa00003e09  code: ff 15 b1 41 00 00 48 85 c0
   ```
   That's `call qword ptr [rip + 0x41b1]` followed by `test rax, rax`.
   It's the standard PIE/PLT indirect call through a GOT entry.
   The GOT slot contains a non-canonical pointer.

2. **xfwm4 (PID 32) — NULL pointer access**
   ```
   SIGSEGV: null pointer access (pid=32, ip=0xa00093d33, fsbase=0xa118f5828)
   RAX=0x0 RBX=0xa1f0df9d0 RCX=0x80 RDX=0x4
   ```
   RAX is 0 — the previous call returned NULL, and the caller
   dereferenced without checking. Stock libX11 idiom: a helper
   returns NULL on EBADF and the caller assumes success.

Both errors are consistent with **stale page cache content** or
**PCID staleness** delivering wrong bytes during a critical
syscall — the kind of bug recent commits like `fb2d9e5` (page
cache partial page poisoning), `22e3fc7` (VMA merge), and
`b502722` (PCID with generation tracking) have been chasing.

## Run 2: non-determinism

A second run produced a different failure mode: PID 1 prints
`T+5 sleeping` then **stops printing for 10+ seconds** before
the test harness's 5-minute Python timeout fires. No segfaults
visible. The test was still alive but PID 1 wasn't getting
scheduled while the xfce4-session children were spawning.

So we have **two intermittent failure modes from the same
underlying instability**:
- User-mode page contents are wrong (when the timing wins races)
- PID 1 is starved while children consume the CPU (when the
  scheduler loses races)

This is the exact same intermittent-Xorg-SIGSEGV story as
blog 151. The `resume_boosted` fix from that blog got us this
far — `xfwm4_running` was 1/4 sometimes — but it didn't close
the case.

## Three quick wins this session

### 1. Lockdep panic in PID 1 cleanup (kernel/process/process.rs)

The very first run surfaced a lockdep panic during PID 1's exit
sequence (after the test had already printed `TEST_END 2/4`):

```
LOCKDEP: lock ordering violation on CPU 0!
Acquiring: SCHEDULER (rank 30, addr 0xffff800002c7f7c0)
While holding: rank 40 (addr 0xffff800002c70160)
Backtrace:
  Process::exit_group → Process::exit → Process::send_signal
  → Process::resume → SpinLock<Scheduler>::lock
```

The bug is in PID 1's "kill all remaining processes before halting"
loop:

```rust
let all_pids: Vec<PId> = PROCESSES.lock().keys().cloned().collect();
for pid in all_pids {
    if pid != PId::new(1) {
        if let Some(proc) = PROCESSES.lock().get(&pid).cloned() {
            proc.send_signal(SIGKILL);
        }
    }
}
```

Rust's drop scoping keeps the temporary `PROCESSES.lock()` guard
alive until the **end of the `if let Some(proc)` body**, not just
the end of the let statement. So `proc.send_signal(SIGKILL)` runs
while `PROCESSES` (rank 40) is still held — and `send_signal →
resume → SCHEDULER.lock` (rank 30) violates the lockdep order.

Fix: two-phase. Snapshot `Vec<Arc<Process>>` inside an explicit
inner block (lock dropped at `}`), then iterate the owned vec
without holding any process-table lock:

```rust
let to_kill: Vec<Arc<Process>> = {
    let table = PROCESSES.lock();
    table.iter()
        .filter(|(p, _)| **p != PId::new(1))
        .map(|(_, proc)| proc.clone())
        .collect()
};
for proc in to_kill {
    proc.send_signal(SIGKILL);
}
```

This was a latent bug — never triggered before because earlier
test_xfce runs got there with fewer surviving children, or because
lockdep wasn't catching it under the right contention pattern.
Either way it's now correct and the panic is gone in run 1.

### 2. Auto-strace-PID-7 was destroying test-xfce performance

`kernel/syscalls/mod.rs` had a debug aid that auto-enabled strace
on PID 7 the first time it issued a syscall:

```rust
if pid == 7 && STRACE_PID.load(...) == 0 {
    STRACE_PID.store(7, ...);
}
```

The intent was "trace xterm during twm tests" — when twm is
skipped, PID 7 is the first interesting child. The reality:
**every syscall on PID 7 emits a `warn!` to serial**, and serial
is the slowest path in the kernel. Traced processes ran ~100×
slower. With test_xfce, PID 7 ends up being one of the early
sh_run() children of the test harness — and once strace turned
on, everything ground to a crawl. The 149-line strace blob in
my first run's log was that auto-strace firing.

Fix: removed the auto-set. Strace is now opt-in via
`set_strace_pid()` only. The log dropped from 149 lines to ~80,
test_xfce ran ~3× faster, and the actual bug stopped being
masked by the slowdown.

### 3. test_xfce.c per-second sleep prints

The harness was doing `sleep(15)` inside Phase 5 with no progress
output, so the only way to tell whether the test was hung at
"start xfce4-session", "sleep finished", or "components check"
was to look at QEMU termination time. Changed to:

```c
for (int s = 0; s < 15; s++) {
    sleep(1);
    printf("  T+%d sleeping\n", 2 + s); fflush(stdout);
}
```

That's how we now know run 2 hung at `T+6`: PID 1 printed T+6
then stopped — five seconds later the harness killed QEMU.

## What this leaves us with

A clear baseline:
- **Test-xfce reaches Phase 5 reliably** and runs the components check.
- **xfwm4 / iceauth crash intermittently with NULL deref + GP fault**
  consistent with stale user-mode page contents.
- **PID 1 is also intermittently starved** during the xfce4-session
  startup storm, even after the resume_boosted fix.
- **kxserver smoke tests are unaffected** (8/8 still pass) — they
  run a static musl binary on the host, no Kevlar dependency.
- **Threading regression unchanged** (14/14 PASS).
- **Lockdep panic during halt is fixed.**

The next two sessions need to chase the two open questions
separately:
1. **Stabilize PID 1 / scheduler under load** (task #24)
2. **Find the user-mode page corruption** (task #25)

Both are kernel work, not kxserver work. The kxserver project
remains the diagnostic baseline — when we want to know "is xterm
hanging on a missing X opcode or a kernel bug?", we run xterm
against kxserver to rule out the X protocol side, then we
**know** the bug is kernel-side. That's the whole reason we
built kxserver, and now it gets to do its job.

## Files changed

- `kernel/process/process.rs` — PID 1 cleanup loop two-phase fix.
- `kernel/syscalls/mod.rs` — disabled auto-strace-PID-7.
- `testing/test_xfce.c` — added per-second sleep prints.
