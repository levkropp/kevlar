# Blog 176: XFCE Scheduler Fix + Three Remaining Kernel Bugs

**Date:** 2026-04-13

## Where we landed

After three instrumented `make test-xfce` runs, we have a much
clearer picture of what's blocking XFCE on Kevlar.

**Scheduler fix: `enqueue_front` now picks the least-loaded queue**

Blog 151 added `resume_boosted()` so timer-based wakeups (nanosleep,
sleep_ms) went to the front of the scheduler run queue. That fixed
the worst case of PID 1 starvation under the SMP twm test, but the
fix wasn't complete — `enqueue_front` inside the scheduler pushed
the boosted PID to the **calling CPU's** queue, not the least-loaded
one. In the XFCE startup load this matters because:

- The timer ISR fires on whichever CPU takes the tick.
- Boosted PIDs all land in front of that CPU's queue.
- Work stealing pops only from the **back** of remote queues.
- So a PID 1 boosted to the front of a busy CPU's queue is
  invisible to the other CPU's stealer until the current process's
  30ms quantum ends — and worse, multiple boosted enqueues can
  chain behind each other, turning a single 30ms gap into
  second-long stalls.

```rust
// kernel/process/scheduler.rs
fn enqueue_front(&self, pid: PId) {
    // Least-loaded queue across online CPUs, not calling-CPU's queue.
    let n = (num_online_cpus() as usize).min(MAX_CPUS);
    let mut best_cpu: usize = 0;
    let mut best_len = usize::MAX;
    for c in 0..n {
        let len = self.run_queues[c].lock().len();
        if len < best_len { best_len = len; best_cpu = c; }
    }
    self.run_queues[best_cpu].lock().push_front(pid);
    RUNQUEUE_LEN.fetch_add(1, Ordering::Relaxed);
}
```

Impact: one run of test-xfce now reaches `T+16 sleeping` reliably
and passes `xfwm4_running` (the first XFCE component that was
failing intermittently). Two runs still hit other bugs
described below.

## Lockdep panic in PID 1 cleanup — fixed

The very first instrumented run surfaced a lockdep panic during
PID 1's halt sequence:

```
LOCKDEP: lock ordering violation on CPU 0!
Acquiring: SCHEDULER (rank 30)
While holding: rank 40 (PROCESSES)
```

The bug was in `Process::exit` for PID 1:

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

Rust's drop scoping kept the `PROCESSES.lock()` guard alive through
the `if let Some(proc) = ...` body, so `send_signal → resume →
SCHEDULER.lock` ran with PROCESSES (rank 40) still held, violating
the rank-30 < rank-40 order.

Fix: two-phase, snapshot Arcs first in an inner block.

## Auto-strace-PID-7 was destroying test-xfce — disabled

`kernel/syscalls/mod.rs` had a debug aid that auto-enabled strace
the first time PID 7 issued a syscall. The goal was to trace xterm
during twm tests; the result in test-xfce was that **every syscall
on PID 7 (which happens to be an early sh_run child) emitted a
`warn!` to serial**. Serial is the slowest path in the kernel;
traced processes ran ~100× slower, and the 149-line strace blob in
my first diagnostic run was masking whatever was really going on.

Removed the auto-set. Strace is now opt-in via `set_strace_pid()`.

## test_xfce.c: per-second sleep prints

The harness was doing `sleep(15)` inside Phase 5 with no progress
output. Replaced with `for (int s = 0; s < 15; s++) { sleep(1);
printf(" T+%d sleeping\n", 2+s); }` — which is how we know the
test now reaches T+16 reliably in good runs.

## OOM visibility

Two `debug_warn!` call sites in the page fault handler were
silently turning page-allocator failures into SIGKILL on user
processes — `debug_warn!` only prints in `debug_assertions` builds
(i.e., never in release). Converted three OOM-on-CoW paths to
plain `warn!` so future investigations can see them.

## The three remaining bugs blocking 4/4

### Bug A — dbus-daemon (PID 24) dies with SIGKILL in every run

~~Every test-xfce run shows exactly one line:~~

**Update: this is NOT a bug.** After instrumenting `send_signal`
with a log of sender and target:

```
SIGKILL: from pid=19 ("dbus-daemon --session --address=unix:path=/tmp/.")
         to   pid=24 ("dbus-daemon --session --address=unix:path=/tmp/.")
PID 24 (dbus-daemon --session …) killed by signal 9
```

PID 19 and PID 24 **both have the same cmdline** — dbus-daemon's
internal fork/kill pattern. The main dbus-daemon (PID 19) forks a
helper child (PID 24) during startup (probably for auth setup or
the print-address mechanism) and SIGKILLs the helper when done.
It's normal dbus behavior and always has been.

Every previous "intermittent dbus crash" we chased in blogs 147,
150, 151 may have been the same red herring — we saw SIGKILL, our
kernel dutifully printed "killed by signal 9", and we assumed
something was killing dbus. Task #26: **resolved as noise**.

The real bug is further down the xfce4-session startup sequence.

### Bug B — xfce4-session SIGSEGV, xfwm4 NULL deref

Run 1 surfaced:
```
SIGSEGV: null pointer access (pid=32, ip=0xa00093d33, fsbase=...)
  RAX=0x0  call site (ret_addr-8..ret_addr): 48 89 df e8 03 fe 02 00
PID 32 (xfwm4) killed by signal 11
```
and earlier:
```
GENERAL_PROTECTION_FAULT pid=31 ip=0xa00003e09
  code: ff 15 b1 41 00 00 48 85 c0
```
The second is an indirect call through a PIE/PLT GOT entry that
contains a non-canonical pointer. The first is a library helper
that returned NULL and its caller didn't check. Both look like
**stale user-mode page contents** — either page cache feeding
stale bytes during a file-mapped text-segment fault, or PCID not
flushing a stale TLB entry after an unmap.

Recent kernel work has been chasing exactly this symptom class:

- `fb2d9e5` Fix page cache partial page poisoning
- `22e3fc7` VMA merge disabled
- `d9c2770` VMA merge restricted to brk heap
- `b502722` PCID with generation tracking
- `37a65b2` SIGSEGV delivery for write faults on RO present pages

The bug is still present under XFCE load. Task #25 is to find it.

### Bug C — Kernel page fault at rip=9, vaddr=9

Run 3 panicked:

```
panicked at platform/x64/interrupt.rs:490:5:
page fault occurred in the kernel: rip=9, rsp=ffff800038a9df18, vaddr=9
```

`rip=9` means the CPU was executing an instruction at virtual
address 9 — i.e., we jumped to near-NULL. That only happens when a
kernel indirect call loads a bogus function pointer off the kernel
stack (corrupt `call [rbp+off]` or `ret` from a corrupted return
address). The backtrace is mangled (`__kernel_image_end` offset
0x72702065607a8f3e — ASCII for `" proce "` + more text — i.e., a
**string literal is on the kernel stack where a return address
should be**). This is a kernel stack overflow or kernel
return-address clobber.

Same root class as bugs A and B but now hitting kernel code.
Task #27.

## Test state summary

| Run | mount | xfwm4 | panel | session | Notes |
|-----|-------|-------|-------|---------|-------|
| 1   | PASS  | PASS  | FAIL  | FAIL    | xfce4-session SIGSEGV, xfwm4 NULL deref in first pass |
| 2   | PASS  | ?     | ?     | ?       | PID 1 hangs at T+5 after dbus SIGKILL |
| 3   | PASS  | ?     | ?     | ?       | Kernel panic at rip=9 vaddr=9 |

**Non-deterministic,** but with a clear floor (mount_rootfs + at
least xfwm4 in the best case) and a ceiling bounded by three
separate bugs.

## What's next

1. **Task #26 (new)** — Instrument every `exit_by_signal(SIGKILL)`
   site and find where dbus dies. Probably a syscall error path
   that panics the process via SIGKILL instead of returning EFAULT.
2. **Task #25** — Find the user-mode page corruption root cause.
   Candidates: PCID generation race, page cache partial page poison
   escaping, VMA merge edge case, CoW refcount bug.
3. **Task #27 (new)** — Investigate the kernel stack overflow /
   return-address clobber surfaced by run 3.

All three are separate failure modes that trace back to the same
rough area: user-mode virtual memory management under high
fork/exec/mmap load. The test-xfce harness gives us a reliable
reproducer, the scheduler fix gives us visibility into when things
actually happen vs when PID 1 was just starved, and the three bugs
can now be attacked independently.

## Regression runs

- `make test-threads-smp` — **14/14 PASS**
- kxserver phase smoke tests 4..11 — **8/8 PASS**
- `make test-xfce PROFILE=balanced` — **1–2/4, non-deterministic,
  best case xfwm4_running**
- Lockdep panic at halt — **fixed**
- Auto-strace performance sink — **fixed**

## Files changed

- `kernel/process/scheduler.rs` — `enqueue_front` → least-loaded queue.
- `kernel/process/process.rs` — PID 1 cleanup two-phase fix, spurious sigreturn warn.
- `kernel/mm/page_fault.rs` — 3 `debug_warn!` → `warn!` for OOM visibility.
- `kernel/syscalls/mod.rs` — disabled auto-strace PID 7.
- `testing/test_xfce.c` — per-second sleep prints.
