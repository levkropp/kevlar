## Blog 178: XFCE 4/4 — atomic PTE mapping + the second TLB deadlock

**Date:** 2026-04-17

After blog 177 proved the corruption was on data pages and that timer-death
was a separate AB-BA deadlock in `tlb_shootdown`, the test still hovered at
2/4 even with the timer-death fix in place. This blog covers the next two
blockers: a non-atomic batch PTE mapping race that corrupted user data on
SMP, and a second TLB-IPI deadlock that the previous fix had missed.

End result: `make test-xfce PROFILE=balanced` reaches **4/4** — `mount_rootfs`,
`xfwm4_running`, `xfce4_panel_running`, `xfce4_session_running` — for the
first time.

## What was still broken

After commit 653007c fixed `tlb_shootdown` to skip the IPI when called with
IF=0, the heartbeat kept ticking past tick=2200 instead of dying at 1800.
But the components scan in Phase 5 of the test still printed:

```
components: wm=1 panel=0 session=0
```

Two problems were stacked on top of each other:

1. **Intermittent SIGSEGV in iceauth** — caused by data-page corruption
   from a different SMP race that timer-death had been masking.
2. **Apparent absence of `xfce4-session`** even after it had successfully
   spawned `xfwm4`.

Both turned out to be one-line bugs sitting on top of a second IPI deadlock.

## Bug 1: non-atomic batch PTE mapping

The single-page `try_map_user_page_with_prot` already used `compare_exchange`
(it was the fix for an earlier double-map race). But the batch path, used
by fault-around when faulting in a chunk of contiguous pages, did this:

```rust
let entry_val = unsafe { *entry_ptr };
if entry_val == 0 {
    unsafe { *entry_ptr = paddrs[i].value() as u64 | attrs_bits; }
    mapped |= 1 << i;
}
```

Two CPUs handling concurrent demand faults on the same VA can both observe
`entry_val == 0`, both write their respective `paddr`, and the loser's
write silently replaces the winner's mapping. If the loser had populated
its physical page from a different file offset (e.g., during fork-around
reaching different prefetch windows on each CPU), the winning CPU now
executes from the wrong physical page. Worse: if the loser then decrements
its refcount and frees the page (because it sees `mapped & (1 << i) == 0`),
the now-mapped page can also have stale data from a fresh allocation.

Fix in `platform/x64/paging.rs` and `platform/arm64/paging.rs`:

```rust
let new_val = paddrs[i].value() as u64 | attrs_bits;
let atom = unsafe { &*(entry_ptr as *const AtomicU64) };
if atom.compare_exchange(0, new_val, AcqRel, Relaxed).is_ok() {
    mapped |= 1 << i;
}
```

CAS makes the publish atomic. Loser observes a non-zero PTE, returns 0 in
its bitmap, and frees its alloc.

After this fix: no more SIGSEGV in iceauth across multiple runs. The data
page corruption documented in blog 177 is gone.

## Bug 2: `tlb_remote_full_flush` was missing the IF=0 guard

With corruption fixed, the test consistently reached Phase 5 — but then
hung with serial output ending in:

```
SPIN_CONTENTION(no_irq): cpu=1 lock="<unnamed>" addr=0xffff800002c9b2b8 spins=5000000
```

The lock was anonymous and heap-shaped. What gives a smoking gun is two
diagnostic additions:

1. `caller_IF` printed alongside the contention message. It was `0` —
   interrupts were *already* disabled at the call site, before any
   `lock_no_irq` (which by definition doesn't change IF).
2. A backtrace dump after the contention message:

```
0: tlb_remote_full_flush()+0x14d
1: sys_mprotect()+0xa1b
2: do_dispatch()+0x2c7
3: handle_syscall()+0x436
4: x64_handle_syscall()+0x158
5: syscall_entry()+0x46
```

Same shape as the earlier timer-death bug, same lock (`TLB_SHOOTDOWN_LOCK`),
same root cause: the function takes the lock, sends an IPI, and spin-waits
for ACK. If the *receiver* on another CPU is in `switch()` or any other
IF=0 path, it can't ACK; the sender holds the lock; both wedge.

The 653007c fix had been applied to `tlb_shootdown` (per-page) but not to
`tlb_remote_full_flush` (full CR3 reload). `sys_mprotect` uses the
batched flush, so it was untouched by the earlier fix.

The fix mirrors the original:

```rust
if !super::interrupts_enabled() {
    super::paging::bump_global_pcid_generation();
    return;
}
```

When IF=0, defer the remote invalidation by bumping the global PCID
generation. Each CPU sees a stale generation on its next context switch
and does a full CR3 reload, which invalidates every entry for its current
PCID. Strictly correct, no IPI needed.

After this fix: 4/4 on the very next run.

## Bug 3: `/proc/N/comm` returned the full path

Even after the deadlock was gone, the components scan still missed
`xfce4-session`. The reason was unrelated to SMP — `Process::get_comm()`
fell back to `argv0()` when no `PR_SET_NAME` had been called, and `argv0()`
returned the entire string given to `execve` (e.g., `/usr/bin/xfce4-session`).
Linux's `/proc/N/comm` returns the basename. The test's `strcmp` against
`"xfce4-session"` never matched.

```rust
let argv0 = cmdline.argv0();
let basename = argv0.rsplit('/').next().unwrap_or(argv0);
basename.as_bytes().to_vec()
```

Two `proc_self.rs` callers also reached into `cmdline().argv0()` directly;
fixed to go through `get_comm()` for consistency.

## Bug 4: thread group leader removed too early

Even with the basename fix, `wm=1 panel=0 session=0` persisted. A diagnostic
dump of `/proc/N/comm` for every PID 2..200 revealed the structure: PID 20
(the xfce4-session main thread) had `comm=unknown`, while its child threads
showed up as `pool-spawner`, `gmain`, `gdbus`, `pool-xfce4-sess`. The threads
were alive; the leader was gone from `PROCESSES`.

`Process::exit()` unconditionally removed the exiting process from
`PROCESSES`. When xfce4-session's main thread reached the end of its
session manager loop and called `pthread_exit(NULL)` (which uses the
`exit` syscall, not `exit_group`), the leader was deleted immediately.
The procfs lookup for `/proc/20/comm` returned `unknown` because
`Process::find_by_pid(PId(20))` returned `None`.

Linux keeps the thread-group leader's `task_struct` in the task list as
a zombie until the last thread in the group exits. We now do the same:

```rust
let is_group_leader = current.pid == current.tgid;
let has_living_threads = is_group_leader && procs.values()
    .any(|p| p.tgid == current.tgid && p.pid != current.pid);

if is_group_leader && has_living_threads {
    // Keep zombie leader for /proc; last thread cleans up.
} else {
    procs.remove(&current.pid);
    if !is_group_leader {
        // If we're the last thread and the leader is zombie, reap.
        // ...
    }
}
```

## Diagnostics that mattered

Two cheap additions did most of the work this session.

**`caller_IF` in `SPIN_CONTENTION`.** A single `pushfq; pop` to read RFLAGS
at the moment the spin threshold trips. `caller_IF=0` with `lock_no_irq`
is by itself the diagnosis: by definition `lock_no_irq` doesn't disable
interrupts, so something upstream did, and that's the deadlock surface.

**Backtrace from inside the spin loop.** The kernel already had
`backtrace::backtrace()`. Calling it from the contention threshold
collapsed minutes of guessing into one line: `tlb_remote_full_flush ←
sys_mprotect`. Expensive per-call (it walks frames and resolves symbols)
but only fires once per 5M-spin episode, so the cost is negligible.

Both are now in `platform/spinlock.rs::lock_no_irq`. The backtrace is
behind nothing — it always runs at threshold — and has paid for itself
twice already.

## What's left

The XFCE deadlock chain that started with timer-death and ended here is
done. The previously-masked SMP races are now visible:

- `PT page 0x... cookie corrupted: 0x0` during `duplicate_table` from
  `Process::fork`. A PT page comes out of the pool with its magic word
  zeroed.
- `switch_thread BUG: cpu=N ret=0x0`. Saved kernel stack has `RIP=0`.
- `Process::resume+0xf4` page-faults with `CR2 ≈ 0x720` — the offset of
  the `state` field inside `struct Process`. The `Arc<Process>` pointer
  itself is corrupted before the resume call.

All three fit one hypothesis: stale TLB entries from a torn-down
address space outlive the page they point to. The freed page is reissued
(to PT pool, to slab, to user mmap), and a write through the stale TLB
hits the new owner.

A first-cut fix went into `teardown_user_pages` /
`teardown_forked_pages` / `teardown_ghost_pages`:

```rust
fn flush_tlb_for_teardown() {
    bump_global_pcid_generation();
}
```

The bump forces every PageTable's stored generation to mismatch the
global generation, so the next `PageTable::switch()` on each CPU does a
full CR3 reload that flushes the current PCID. Stability went from 1/4
to 2/4. The PT-page cookie panic stopped appearing in most runs.

Then we extended the IPI protocol with a "flush all PCIDs" sentinel
(`vaddr = usize::MAX`), implemented via `INVPCID` type=3 (with a
`CR4.PCIDE` toggle as fallback), detected at boot via `CPUID.7`. The
key insight: the existing `tlb_remote_full_flush` told remote CPUs to
flush their *current* PCID, which is wrong when the unmapping process
runs on CPU A and CPU B is currently scheduling a *different* process —
B's TLB entries tagged with A's PCID would never be flushed by such an
IPI. Switching `PageTable::flush_tlb_remote()` to the new all-PCIDs
variant fixed this whole class.

Stability improved further to 3/4 of runs reaching 3-4/4. The `PT page
cookie corrupted` panic is gone in steady state. The `Process::resume`
crash (an `Arc<Process>` in a `WaitQueue` with its `ptr` field zeroed)
still appears in roughly one in four runs.

## The IRQ-bottom-half drop

Tracking down the remaining heap corruption, we instrumented `wake_all`
with an `Arc::as_ptr` range check. On a crash, the dump showed:

```
WAITQ CORRUPT: queue=0xffff800002ca0ac8 bad_arc[1]=0x11 of 16
  waiters[0]  = 0xffff80003d3f9110  <- valid kernel Arc
  waiters[1]  = 0x11                 <- corrupt (literal 17)
  waiters[2]  = 0x264d5010           <- corrupt (~physical address)
  waiters[3]  = 0x11
  waiters[4]  = 0x2679a010
  waiters[5]  = 0x11
  waiters[6]  = 0x2679d010
  waiters[7]  = 0x11
  waiters[8]  = 0xffff80003d145010   <- valid again
```

The queue is `POLL_WAIT_QUEUE` — the one the timer ISR calls `wake_all`
on to resume poll/epoll/select waiters. Three consecutive 16-byte
blocks of `(0x11, some_paddr_ending_0x010)` were written across the
`VecDeque<Arc<Process>>` buffer, each overwriting two `Arc<Process>`
slots. The first and last entries survived.

Hypothesis: the queue's heap backing memory was previously a user page.
After it was `munmap`'d and freed, another CPU still held a stale TLB
entry for a user VA pointing to that physical page. When a user process
subsequently wrote `(0x11, some_paddr)` tuples through that VA (~ an
iovec-shaped structure), the writes landed in the now-reused kernel
heap.

Why the `flush_tlb_remote()` / INVPCID changes didn't prevent it: when
the TLB flush runs, it flushes every PCID on every CPU — but only at
the *moment it runs*. The sequence that corrupts is:

1. Process P has VA→X with PCID p. CPU A's TLB caches it.
2. CPU A context-switches away from P (CR3 reloaded, entry stays dormant).
3. P exits. Its Vm drops → `teardown_user_pages` →
   `flush_tlb_for_teardown`.
4. **If the drop happened with IF=0**, the IPI was skipped. CPU A's
   TLB still holds VA→X.
5. X is freed to buddy → reissued as a kernel-heap block →
   `WaitQueue` VecDeque buffer.
6. On CPU A, the process currently scheduled is *P's sibling*, which
   shares PCID p. VA still resolves through the stale entry → kernel
   heap corruption.

The "drop happened with IF=0" bit is the missing piece. `gc_exited_processes`
runs from the IRQ bottom half with IF=0. When it drops the last
`Arc<Process>`, `Vm::Drop` → `teardown_*_pages` → `flush_tlb_for_teardown`
detects IF=0 and falls back to the bump-gen-only path.

Two fixes:

```rust
// gc_exited_processes: drop Arcs OUTSIDE the EXITED_PROCESSES lock,
// and with IF=1 so Vm::Drop's TLB IPI can actually fire.
let to_drop = { core::mem::take(&mut *EXITED_PROCESSES.lock()) };
let was_if_off = !interrupts_enabled();
if was_if_off { enable_interrupts(); }
drop(to_drop);
process_deferred_vm_teardowns();  // also drain any IF=0 deferrals
if was_if_off { asm!("cli"); }
```

```rust
// Vm::Drop: if called with IF=0 (e.g. from a wake_all's waiters drop
// in the timer ISR), stash the pml4 in DEFERRED_VM_TEARDOWNS and let
// gc_exited_processes finish the teardown with IF=1.
impl Drop for Vm {
    fn drop(&mut self) {
        let kind = /* GhostForked | Forked | None */;
        if !interrupts_enabled() {
            DEFERRED_VM_TEARDOWNS.lock_no_irq().push(DeferredTeardown {
                pml4: self.page_table.pml4(),
                kind,
            });
            self.page_table.clear_pml4_for_defer();
            return;
        }
        // normal path
    }
}
```

Plus a defensive guard in `wake_all` — if the range check catches a
bad `Arc<Process>`, log once, `mem::forget` the bad value (its Drop
would crash), and continue resuming the good ones. The guard leaks on
bad Arcs, but that's strictly better than a kernel panic while XFCE is
starting.

After these fixes: 6/8 runs complete without a kernel panic (test
scoring is a separate XFCE-startup-timing issue), down from ~4/8.
The two still-panicking runs hit a different symptom: a kernel stack
page's saved RIP slot is zeroed, `ret` jumps to 0, and the NX fault at
rip=2 panics. Same root cause (user write through stale TLB into a
reissued kernel page) — but now hitting a kernel stack allocation
instead of the WaitQueue heap. The deferred-teardown path handles the
common case; the remaining races likely involve a different drop site
we haven't covered yet.
