# Blog 160: Fixing Benchmark Regressions — PCID, Lock-Free Rlimits, Flock, FD Hints

**Date:** 2026-04-13

## Background

The full 53-benchmark KVM comparison against Linux showed 8 regressions
worse than 1.10x.  Five of those were worse than 1.20x:

| Benchmark     | Linux  | Kevlar | Ratio |
|---------------|--------|--------|-------|
| pipe_pingpong | 2.7us  | 4.5us  | 1.64x |
| prlimit64     | 145ns  | 188ns  | 1.30x |
| getrlimit     | 148ns  | 190ns  | 1.28x |
| flock         | 387ns  | 481ns  | 1.24x |
| dup_close     | 282ns  | 341ns  | 1.21x |

By the end of the session, all 52 other benchmarks were **faster than Linux**
and only pipe_pingpong remained as a near-threshold regression (1.20x).
The biggest win was enabling PCID with proper generation-based TLB
invalidation, which dramatically improved nearly every benchmark.

## Fix 1: Lock-Free Rlimits (getrlimit, prlimit64, dup_close)

**Root cause:** `Process::rlimits()` copied the *entire* 256-byte table
(`[[u64; 2]; 16]`) under a SpinLock, even when callers only needed a
single pair.  Three separate hot paths hit this:

1. `sys_getrlimit` — copies all 16 entries, reads one
2. `sys_prlimit64` — delegates to sys_getrlimit
3. `alloc_fd` — copies all 16 entries to read RLIMIT_NOFILE

**Fix:** Replaced `SpinLock<[[u64; 2]; 16]>` with `AtomicRlimits` — 32
`AtomicU64` values indexed as `[cur0, max0, cur1, max1, ...]`.

```rust
struct AtomicRlimits {
    vals: [AtomicU64; 32],
}

impl AtomicRlimits {
    fn get(&self, idx: usize) -> [u64; 2] {
        [self.vals[idx * 2].load(Ordering::Relaxed),
         self.vals[idx * 2 + 1].load(Ordering::Relaxed)]
    }
}
```

Reads are now 2 atomic loads (~2ns) instead of spinlock acquire +
256-byte memcpy + release (~20ns).  Writes (setrlimit only) are 2
atomic stores — acceptable because rlimits are rarely written.

Additionally, `sys_getrlimit` was writing the result as two separate
8-byte usercopies.  Combined into a single `buf.write(&pair)` call,
saving one `access_ok` check.

**Result:** getrlimit 190ns → 141ns (0.95x), prlimit64 188ns → 139ns
(0.96x).

## Fix 2: Lock-Free inode_key for flock

**Root cause:** Every `flock()` call invoked `opened_file.inode().stat()`
just to extract two fields: `dev_id` and `inode_no`.  For tmpfs files,
`stat()` acquires **four spinlocks** (mode, uid, gid, data size), six
atomic timestamp loads, and builds a 176-byte Stat struct — all to read
two immutable fields.

**Fix:** Added `inode_key()` to the `FileLike` trait with a default
implementation that falls back to `stat()`.  Overridden in tmpfs `File`
to read the immutable `self.stat.dev` and `self.stat.inode_no` directly
— zero locks, zero atomics.

```rust
// FileLike trait (default)
fn inode_key(&self) -> Result<(usize, u64)> {
    let st = self.stat()?;
    Ok((st.dev.as_usize(), st.inode_no.as_u64()))
}

// tmpfs File (override — lock-free)
fn inode_key(&self) -> Result<(usize, u64)> {
    Ok((self.stat.dev.as_usize(), self.stat.inode_no.as_u64()))
}
```

**Result:** flock 481ns → 340ns (0.88x) — now faster than Linux.

## Fix 3: FD Allocation Hint for dup_close

**Root cause:** `alloc_fd()` always scans from FD 0 (POSIX: lowest
available).  In the dup_close benchmark, FDs 0-3 are occupied (stdin,
stdout, stderr, /dev/null), so every `dup()` call checks 4 occupied
slots before finding FD 4 free.

**Fix:** Added `lowest_free_hint` to `OpenedFileTable`.  `alloc_fd()`
starts scanning from the hint.  `close()` updates the hint when a
lower FD is freed:

```rust
pub fn close(&mut self, fd: Fd) -> Result<()> {
    // ... close the file ...
    if fd.as_int() < self.lowest_free_hint {
        self.lowest_free_hint = fd.as_int();
    }
    Ok(())
}
```

For the dup_close pattern (dup(3)→4, close(4), repeat): close(4) sets
hint to 4, next dup starts from 4 instead of 0.  Eliminates 4 wasted
iterations per alloc_fd.

**Result:** dup_close 341ns → ~305ns (1.08x marginal, down from 1.21x).

## Fix 4: Pipe wake_one

**Root cause:** Pipe read/write called `wake_all()` on the per-pipe
wait queue after every operation.  `wake_all()` allocates a `Vec` to
collect all waiters before resuming them (to avoid holding the queue
lock across `resume()`).  For the common single-waiter case in
pipe_pingpong, this is a heap allocation per wake.

**Fix:** Renamed `_wake_one()` to `wake_one()` (was dead code) and
changed all pipe wake calls to use it.  `wake_one()` pops a single
waiter from the VecDeque without allocating — perfect for pipes where
at most one reader or writer is sleeping at a time.

Correctness: the pipe wait queue is shared between readers and writers,
but they can't both be sleeping simultaneously (buffer can't be both
full and empty).  `wake_one()` always wakes the right type.

**Result:** pipe_pingpong ~5% improvement (4.5us → 4.2us).

## Fix 5: Redundant Sleep Condition Check

`WaitQueue::sleep_signalable_until` always checks the condition once before
entering the sleep loop.  But the pipe read/write slow path already
verified the buffer state under lock — the re-check is a guaranteed
miss that re-acquires the pipe lock for no reason.

**Fix:** Added `sleep_signalable_until_unchecked` which skips the initial
condition check.  Used from pipe read/write slow paths where the caller
already knows the condition is not met.

Saves one pipe lock acquisition per blocking read/write.

## Fix 6: PCID with Generation-Based Invalidation (the big one)

**Root cause:** Kevlar's PCID support had been disabled since Blog 151's
XFCE SIGSEGV investigation.  `alloc_pcid()` returned 0, meaning every
CR3 write fully flushed the TLB.  On KVM with EPT, each subsequent TLB
miss requires a nested 4×4 page walk — ~50ns per miss.  Context switches
incurred ~1.5us of TLB miss overhead each.

The reason PCID was disabled: a monotonic PCID counter wraps after 4095
allocations.  A new process would reuse a PCID from a dead process, but
stale TLB entries tagged with that PCID could still be cached — causing
memory corruption (the XFCE phantom COW bug).

**Fix:** Replace the monotonic counter with a **generation counter**,
Linux-style.  Each `PageTable` stores a packed 64-bit value: bits [63:12]
= generation, bits [11:0] = PCID.  When allocations exhaust PCID 4095,
the generation increments and PCID resets to 1.

On context switch, `PageTable::switch()` compares the stored generation
against the global generation:

```rust
if my_gen == global_gen {
    // Same generation: TLB entries are valid. Set bit 63 (no-invalidate).
    cr3_write(pml4 | pcid | (1u64 << 63));
} else {
    // Stale generation: flush by writing CR3 without bit 63.
    cr3_write(pml4 | pcid);
    // Update stored generation so subsequent switches are fast.
    self.pcid_gen.store(global_gen | pcid, Relaxed);
}
```

**Key properties:**
- Lock-free allocation via CAS on a single `AtomicU64`
- No per-exit `free_pcid()` bookkeeping — dead PCIDs are implicitly
  reclaimed when the generation advances
- Safe under SMP: each CPU independently checks the generation and flushes
  locally.  Worst case is an unnecessary flush if the global generation
  advances between load and CR3 write.
- The generation field is 52 bits — overflows after ~142M years at 1M
  process creations/sec
- `duplicate_from()` (fork COW path) still allocates a fresh PCID for the
  parent so stale remote-CPU entries are invalidated

The `AtomicU64` allows interior mutability in `switch(&self)` — only the
CPU switching to a process calls its `switch()`, and the context-saved
spinwait guarantees no concurrent execution.

## Results

A single change (PCID generation tracking) made **nearly every benchmark
faster than Linux**, not just the pipe pingpong.  Timer IRQs and
scheduler preemptions are the hot paths that preserve TLB entries with
PCID.

| Benchmark     | Before       | After          | Status         |
|---------------|-------------|----------------|----------------|
| flock         | 481ns 1.24x | 251ns 0.65x    | **FIXED**      |
| getrlimit     | 190ns 1.28x | 109ns 0.74x    | **FIXED**      |
| prlimit64     | 188ns 1.30x | 109ns 0.75x    | **FIXED**      |
| dup_close     | 341ns 1.21x | 237ns 0.84x    | **FIXED**      |
| pipe_pingpong | 4.5us 1.64x | 3.3us 1.20x    | Near-threshold |

**Overall summary:**
- Before: 18 faster, 12 OK, 15 marginal, 8 regressions
- After: **52 faster**, 0 OK, 0 marginal, **1 regression** (pipe_pingpong)

Huge micro-benchmark wins include getpid (119→72ns), mmap_fault
(2.0us→93ns), mmap_munmap (1.9us→327ns), mprotect (2.6us→1.3us),
socketpair (2.9us→1.1us), exec_true (120us→81us), shell_noop
(154us→117us), sort_uniq (1.3ms→933us), and file_tree (75us→34us).

## Correctness Verification

All 14 SMP threading regression tests pass, including the fork/exec
churn tests (`fork_from_thread`, `pipe_pingpong`, `thread_storm`) that
previously triggered the phantom COW bug with the monotonic PCID
counter.  The generation-based invalidation correctly prevents stale
TLB entries from persisting across PCID reuse.
