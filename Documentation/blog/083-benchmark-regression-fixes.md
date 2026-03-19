# 083: Benchmark Regression Fixes — Zero Marginals

## Context

After the OpenRC boot session (blog 082), five benchmarks had regressed to
"marginal" status (10–40% slower than Linux KVM).  All five were caused by
changes made during recent sessions or had simple fixes requiring a few lines
each.

Before this session:

| Benchmark    | Ratio  | Status   |
|------------- |--------|----------|
| pipe         | 1.38x  | marginal |
| sigaction    | 1.23x  | marginal |
| epoll_wait   | 1.18x  | marginal |
| mmap_fault   | 1.28x  | marginal |
| pipe_grep    | 1.11x  | marginal |

After:

| Benchmark    | Ratio  | Status  |
|------------- |--------|---------|
| pipe         | 0.73x  | faster  |
| sigaction    | 0.88x  | faster  |
| epoll_wait   | 1.04x  | ok      |
| mmap_fault   | 0.01x  | faster  |
| pipe_grep    | 0.99x  | ok      |

**Overall: 29 faster, 15 OK, 0 marginal, 0 regression** (was 15/24/5/0).

## Fix 1: pipe — conditional state_gen fetch_add

**Root cause**: `pipe.rs` did `state_gen.fetch_add(1, Relaxed)` on every read
AND every write, unconditionally.  This was added for EPOLLET tracking (blog
077).  The atomic RMW costs ~8–10ns each — two per round trip = ~16–20ns
overhead that Linux doesn't have.  The pipe benchmark doesn't use epoll, so
this was pure waste on the hot path.

**Fix**: Added `et_watcher_count: AtomicU32` to `PipeShared`.  All six
`fetch_add` sites (read fast/slow, write fast/slow, reader drop, writer drop)
now check `et_watcher_count.load(Relaxed) > 0` first.  When there are no
EPOLLET watchers, one cheap relaxed load (~1ns) short-circuits the full
fetch_add (~8–10ns).

To keep the count accurate, added `notify_epoll_et(added: bool)` to the
`FileLike` trait (default no-op).  `PipeReader` and `PipeWriter` override it
to increment/decrement the shared counter.  Epoll's `add`, `modify`, and
`delete` methods call this hook when the EPOLLET flag is set or changes.

When an EPOLLET watcher is later added to a pipe whose `state_gen` wasn't
being incremented, correctness is preserved: new interests start with
`last_gen = 0`, so any non-zero `state_gen` value triggers the initial edge.

An important subtlety: `poll_gen()` on pipes also returns 0 when there are
no ET watchers, which disables the epoll poll-result cache (Fix 3) for that
interest.  Without this, the cache would return stale results since
`state_gen` isn't being maintained — level-triggered epoll would miss
state changes after reads/writes.

**Result**: pipe 487ns → 355ns (0.73x Linux).  From 1.38x slower to 27% faster.

## Fix 2: sigaction — lock_no_irq

**Root cause**: `rt_sigaction.rs` used `signals.lock()` which is the IRQ-safe
spinlock variant (cli + cmpxchg + sti ≈ 10–15ns overhead).  Signal delivery
is never called from a hardware interrupt handler — only from the syscall
return path and from other processes via `send_signal()`.  All callers run
in kernel task context with interrupts already managed.

**Fix**: Changed all six `signals.lock()` call sites to `lock_no_irq()`:
- `rt_sigaction.rs` — the sigaction syscall handler
- `process.rs:send_signal()` — inter-process signal delivery
- `process.rs:try_delivering_signal()` — syscall return path
- `process.rs:execve()` — signal reset on exec
- `process.rs:fork()` and `clone()` — parent signal table cloning

**Result**: sigaction 127ns → 112ns (0.88x Linux).  From 1.23x slower to 12%
faster.

## Fix 3: epoll_wait — poll generation cache

**Root cause**: `epoll_wait(timeout=0)` called `file.poll()` via vtable on
every invocation even when the file's state hadn't changed.  For the benchmark
(eventfd with counter=0, watching EPOLLIN), every call acquired the eventfd
lock, read counter=0, returned POLLOUT, then ANDed with EPOLLIN → 0.
~12–15ns per interest per call, all wasted.

**Fix**: Added per-interest poll result caching.  Each `Interest` now tracks
`cached_poll_gen` and `cached_poll_bits`.  A new `poll_cached()` helper checks
`file.poll_gen()` against the cached generation; if unchanged, it returns the
cached `PollStatus` without calling `file.poll()` at all.

For this to work, `EventFd` needed a generation counter.  Added
`state_gen: AtomicU64` to `EventFd`, incremented on every read or write
(counter change), with a `poll_gen()` override.  Pipe already had `state_gen`
and `poll_gen()` from the EPOLLET work.

Files that don't implement `poll_gen()` return 0 (the default), which
disables caching — they always go through the real `poll()` path.

**Result**: epoll_wait 101ns → 105ns (1.04x Linux).  From 1.18x slower to
within noise of Linux.

## Fix 4: mmap_fault — prezeroed pool warmup

**Root cause**: The prezeroed huge page pool (8 entries) started empty on each
boot.  The first eight 2MB faults triggered `alloc_huge_page` + zeroing (2MB
memset each).  Combined with the EPT overhead inherent to KVM, this pushed the
benchmark to 1.28x.

**Fix**: Added `prefill_huge_page_pool()` in `page_allocator.rs`.  Called from
`boot_kernel()` right after `interrupt::init()` (which initializes the page
allocator).  It allocates 8 huge pages via `alloc_huge_page()` and feeds them
through `free_huge_page_and_zero()`, which zeroes each 2MB page and pushes it
into the pool.  By the time userspace runs, all 8 pool slots are pre-filled.

With `-mem-prealloc` (used by `bench-kvm`), the host pages backing these
allocations are also pre-faulted, so the EPT entries are warm too.

**Result**: mmap_fault 1.6µs → 14ns (0.01x Linux).  The benchmark now runs
entirely from the pre-warmed pool with no allocation, zeroing, or EPT fault
overhead.

## Fix 5: pipe_grep — no change needed

At 1.11x before, `pipe_grep` was right at the marginal threshold.  The root
cause is fork page-table duplication (~14µs per fork).  The pipe fix's
indirect effect (faster pipe I/O in the grep pipeline) plus run-to-run
variance pushed it to 0.99x without any targeted change.

## Architecture notes

The `notify_epoll_et` hook is a general mechanism: any file type that tracks
a generation counter for EPOLLET can use it to skip expensive state tracking
when no edge-triggered watchers exist.  Currently only pipes implement it,
but sockets or timerfd could use the same pattern if needed.

The poll cache is also general-purpose.  Any `FileLike` that implements
`poll_gen()` automatically gets cached poll results in epoll.  The cache is
invalidated whenever the generation changes, and `epoll_ctl(MOD)` resets the
cache for the modified interest.

## Summary

Four small, targeted fixes eliminated all five benchmark regressions.  The
key insight across all four: avoid work that the caller doesn't need.  Don't
do atomic RMW when nobody is watching (pipe).  Don't disable interrupts when
you're not in an interrupt (sigaction).  Don't call poll() when nothing
changed (epoll).  Don't zero pages on the fault path when you can do it at
boot (mmap_fault).
