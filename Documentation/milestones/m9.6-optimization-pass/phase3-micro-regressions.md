# M9.6 Phase 3: Micro-Regressions

**Regressions:** `read_null` (1.2x), `write_null` (1.2x),
`sched_yield` (1.1x), `epoll_wait` (1.1x), `eventfd` (1.1x)
**Target:** All within 1.1x of Linux KVM

## The problem

Five syscalls show small but consistent regressions:

| Benchmark | Kevlar | Linux | Overhead |
|-----------|-------:|------:|---------:|
| read_null | 158ns | 132ns | +26ns |
| write_null | 158ns | 132ns | +26ns |
| sched_yield | 219ns | 196ns | +23ns |
| epoll_wait | 150ns | 136ns | +14ns |
| eventfd | 386ns | 349ns | +37ns |

These are all fast-path syscalls where 20-40ns matters.

## Analysis approach

1. **read_null / write_null** — profile the sys_read and sys_write
   paths for /dev/null.  The 26ns overhead is likely:
   - fd table lock acquire/release (even lock_no_irq costs ~10ns)
   - File offset update (lseek tracking on /dev/null is pointless)
   - OpenedFile wrapper overhead (Arc clone, options check)

2. **sched_yield** — profile the yield path.  23ns suggests one
   extra lock acquire or unnecessary scheduler state check.

3. **epoll_wait / eventfd** — 14-37ns overhead.  Likely in the
   event notification mechanism or fd readiness checks.

## Potential fixes

### Fix A: /dev/null bypass

Detect /dev/null by inode at open time, store a flag in OpenedFile.
On read/write, skip file offset tracking, skip lock, return immediately.

### Fix B: fd table read lock optimization

If the fd table lock is acquired as a write lock for reads, switch
to a reader lock or lock-free lookup for the common case.

### Fix C: sched_yield fast path

If the current CPU's run queue is empty (common on single-CPU tests),
sched_yield can return immediately without touching the scheduler's
data structures.

## Success criteria

All five benchmarks within 1.1x of Linux KVM.
