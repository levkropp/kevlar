## Blog 192: allocator locks must be IRQ-safe before broad sti can land

**Date:** 2026-04-19

[Blog 190](190-lock-no-irq-preempt-disable.md) closed the
`STACK_REGISTRY` deadlock (syscall holds lock, gets preempted, other
thread on the same CPU wedges on the same lock).  [Blog 191](191-sti-livelock-not-deadlock.md)
found the remaining failure is a livelock rather than a hang.  This
post is one more slice of the onion: with the per-syscall latency
histogram from commit f84193e pointing at the right spot, the real
cause turns out to be classic — allocator locks reached from both
syscall context and IRQ context, when neither is disabling the other.

## What the histogram said

Running `test-xfce --nmi-on-stall 12` with broad `sti` applied and
`addr2line`-ing the stuck RIP surfaced this call chain on CPU 0:

```
core::sync::atomic::atomic_load::<u8>           ← actually spinning here
alloc::raw_vec::RawVecInner::try_allocate_in
kevlar_kernel::interrupt::handle_irq        (kernel/interrupt.rs:59)
interrupt_common
kevlar_ext2::Ext2File::read                 (services/kevlar_ext2/src/lib.rs:2982)
kevlar_kernel::process::process::prefault_writable_segments
```

CPU 0 is mid-syscall inside `Ext2File::read`, an IRQ fired (timer or
virtio — both legal with IF=1 from the syscall body), and the IRQ
handler is trying to `Vec::new()` from the deferred-job path.  The
`atomic_load` is on the `spin::Mutex` that guards the buddy
allocator's heap.  That lock is held by the syscall that just got
interrupted, but it won't ever be released because the syscall is
paused in the IRQ handler and the IRQ handler is paused on the lock.

## Three sites, one class of bug

The lock-held-across-IRQ anti-pattern shows up in three places:

1. **`platform/page_allocator.rs`** — `ZONES`, `PAGE_CACHE`,
   `PREZEROED_4K_POOL`, `HUGE_PAGE_POOL` were all `lock_no_irq`.
   Reached from `alloc_page` (hot path in every syscall that
   touches VM) and from `refill_prezeroed_pages` (interval_work in
   the idle / IRQ-bottom-half path).
2. **`platform/stack_cache.rs`** — `STACK_REGISTRY` and the
   per-size stack caches, same pattern.
3. **`platform/global_allocator.rs`** — the `buddy_system_allocator`
   crate's upstream `LockedHeapWithRescue` wraps its `Heap<N>` in a
   `spin::Mutex`.  That's not IRQ-safe.  Every Rust `Box::new`,
   `Vec::new`, formatter backing buffer, etc. goes through the
   global allocator; if any IRQ handler path allocates while a
   syscall already holds the heap lock, we deadlock.

## The fix

For the first two, a sweeping `.lock_no_irq()` → `.lock()`.  The
cost is a `pushfq` + `cli` + matching `popfq` per acquire, which on
these code paths is dwarfed by the actual allocator work.

For the global allocator, wrap the raw `Heap<ORDER>` in
`kevlar_platform::spinlock::SpinLock` instead of `spin::Mutex`:

```rust
struct KevlarLockedHeap<const N: usize> {
    inner: SpinLock<Heap<N>>,
    rescue: fn(&mut Heap<N>, &Layout),
}

#[global_allocator]
static ALLOCATOR: KevlarLockedHeap<ORDER>
    = KevlarLockedHeap::new(expand_kernel_heap);
```

Our `SpinLock::lock()` disables IF on acquire and restores on drop,
closing the re-entry window.

## What we rolled back

I also tried making `preempt_count` per-thread (save/restore across
`do_switch_thread`) to accompany the allocator fix.  That regressed
`test-xfce` with `TASK CORRUPT (PERSISTENT)` signatures — PID 1
ending up with `saved_rip=0x0` after a sleep-and-resume, suggesting
the new stack layout (9 saved slots instead of 8) interacted badly
with some path I didn't fully audit.  Reverted.  Per-thread
preempt_count can come back on another turn once the whole code
path that reads from a task's saved context is examined.

## Landed, not landed

**Landed (this commit, 4031f2c):**
- `page_allocator` + `stack_cache` + `global_allocator` all IRQ-safe.
- Zero behavioral change with the current IF=0 syscall body — these
  are preemptive fixes for a future broad-sti re-attempt.

**Still not landed:** broad `sti` in `syscall_entry`.  With the
three allocator fixes in tree, the next broad-sti attempt won't hit
the ext2-read-interrupted-by-IRQ-alloc livelock documented in this
blog.  Open question: whether per-thread preempt_count is also
needed, and if so, how to add it without the stack-layout regression.

Kernel still has the blog-188 silent stale-TLB issue on every
syscall, because `flush_tlb_remote` from IF=0 syscall bodies still
degrades to "bump PCID generation, skip the IPI."  Every time that
happens on a heavy mprotect / execve flow, a stale TLB entry
survives long enough to leak a kernel pointer into a freshly
recycled user page.  Rare, measurable, still real.

Status: `test-threads-smp` 14/14, `test-xfce` 3/3 runs complete
(scores 1-4/4, usual userspace variance).  No regressions.
