# From 13µs to 600ns: Three Rounds of KVM Performance Work

Our benchmarks showed getpid taking 13,000 ns per call on KVM — about 65x
slower than native Linux. `read(/dev/null)` was 26 µs, `stat` was 264 µs.
The kernel was functionally correct but unusably slow under virtualization.

Three rounds of targeted optimization, guided by a new profiling
infrastructure we built along the way, brought these numbers down to
near-Linux performance:

| Benchmark | Start | Final | Speedup |
|-----------|-------|-------|---------|
| getpid | 13,000 ns | 279 ns | **47x** |
| read_null | 26,000 ns | 613 ns | **42x** |
| write_null | 28,000 ns | 650 ns | **43x** |
| pipe | 625,000 ns | 91,435 ns | **6.8x** |
| stat | 264,000 ns | 23,785 ns | **11x** |
| open_close | 95,000 ns | 21,188 ns | **4.5x** |

## Round 1: Eliminating VM exits

Under KVM, port I/O (`in`/`out`) and MMIO writes cause VM exits — 1-10 µs
each. We were generating thousands of unnecessary exits per second.

**Serial TX busy-wait**: QEMU's virtual UART is always ready, but we polled
`inb(LSR)` before every character. Each poll is a VM exit. Fix: skip the
poll, write directly.

**VGA cursor updates**: Every character printed to serial was *also* sent to
VGA, where `move_cursor()` does 4 `outb()` calls. For 80 characters of
output: 320 wasted VM exits. Fix: VGA only used at boot.

**Interrupt trace logging**: An unconditional `trace!()` in the interrupt
handler wrote formatted strings to serial on every non-timer IRQ. Fix:
remove; the structured debug event system handles tracing when explicitly
enabled.

**1000 Hz timer**: One PIT interrupt per millisecond, each causing a VM exit
for delivery plus MMIO for EOI acknowledgment. Fix: reduce to 100 Hz (same
30 ms preemption interval, 3 ticks instead of 30).

**APIC spinlock**: Every IRQ did `APIC.lock().write_eoi()` — our SpinLock
disables interrupts, checks for deadlocks, acquires the lock, does the MMIO
write, releases the lock, restores interrupts. On a single-CPU kernel with
interrupts already disabled: pure overhead. Fix: inline the EOI write.

**Signal spinlock per syscall**: Every syscall exit acquired a spinlock to
check for pending signals — even when none were pending. Fix: `AtomicU32`
mirror of the pending bitmask, checked with a relaxed load.

**Result**: getpid went from 13,000 ns to **200 ns**. Everything else
improved 1.5-5x. But we couldn't measure precisely — our clock only had
10 ms resolution.

## Round 2: Nanosecond clock and profiling infrastructure

### TSC calibration

`clock_gettime(CLOCK_MONOTONIC)` was tick-based at 100 Hz — 10 ms
granularity. We calibrated the TSC against PIT channel 2 during early boot:

```rust
// Program PIT channel 2 for ~10ms one-shot
let tsc_start = rdtscp();
while inb(0x61) & 0x20 == 0 { spin_loop(); }  // wait for terminal count
let tsc_end = rdtscp();
let freq = (tsc_end - tsc_start) * PIT_HZ / pit_count;
```

Now `nanoseconds_since_boot()` is a single `rdtscp` instruction with
lock-free atomic reads. Wired into `clock_gettime(CLOCK_MONOTONIC)` for
ns-resolution userspace timing. Also fixed a latent bug where `tv_nsec`
returned total nanoseconds instead of the sub-second component.

### Per-syscall cycle profiler

512-entry fixed array indexed by syscall number, lock-free atomics tracking
total cycles, call count, min, and max per syscall. Two `rdtscp` calls
bracketing `do_dispatch()` — ~10 ns overhead when enabled, zero when
disabled (single atomic bool check).

Enabled via `KEVLAR_DEBUG="profile"`. On init process exit, dumps JSONL:

```
{"nr":39,"name":"getpid","calls":10001,"avg_ns":49,"min_ns":38,"max_ns":9950}
{"nr":0,"name":"read","calls":5032,"avg_ns":12798,"min_ns":11329,"max_ns":126032}
```

The profiler immediately revealed the next bottleneck: every syscall that
touches a file pays ~12 µs for spinlock overhead, while getpid (no locks)
costs only 49 ns. The lock is the problem.

## Round 3: The spinlock backtrace tax

The profiler showed read/write/close all clustered at ~13 µs regardless of
what the actual syscall did. `/dev/null` read returns `Ok(0)` immediately —
the 13 µs was entirely in the surrounding infrastructure.

The culprit was in our `SpinLock::lock()`:

```rust
// In debug builds, EVERY lock acquire:
#[cfg(debug_assertions)]
if is_kernel_heap_enabled() {
    *self.locked_by.borrow_mut() = Some(CapturedBacktrace::capture());
}
```

`CapturedBacktrace::capture()` does:
1. `Box::new(ArrayVec::new())` — **heap allocation**
2. Walk the entire call stack frame by frame
3. Resolve each frame's symbol via the kernel symbol table

This ran on **every lock acquire**, even when uncontended. On a single-CPU
kernel, locks are never contended (contention = deadlock). The backtrace
was only useful when the deadlock detector fired — which never happens in
normal operation.

Fix: remove the per-acquire capture. The deadlock detector still works
(it prints the warning when `is_locked()` is true on entry).

Also removed unconditional `trace!()` calls from `sys_read`, `sys_write`,
and `sys_open` that formatted PID, cmdline, inode Debug, and length on
every call.

**Result**: read dropped from 12,798 to **391 ns** (36x). The profiler
paid for itself immediately.

## The profiler's view of the final state

```
getpid:         49 ns   — pure syscall overhead floor
geteuid:       132 ns   — trivial lookup
read:          391 ns   — fd table lock + dyn dispatch
write:         960 ns   — fd table lock + dyn dispatch + output
close:       1,611 ns   — fd table lock + cleanup
clock_gettime: 1,702 ns — TSC read + usercopy
open:       18,952 ns   — path resolution dominates
stat:       24,164 ns   — path resolution + inode stat
fork:    2,884,536 ns   — page table copy + allocation
```

The gap between getpid (49 ns) and read (391 ns) is ~8x — that's the fd
table spinlock (cli/sti + lock acquire + Arc clone + lock release + sti)
plus a virtual method dispatch through the `FileLike` trait. Closing this
gap would require either lock-free fd table access or amortizing the lock
across multiple operations.

The gap between read (391 ns) and stat (24 µs) is path resolution — the
VFS walk through string comparisons and directory inode lookups. Linux uses
a dcache (directory entry cache) with RCU-protected hash lookups to make
this fast. Building an equivalent is the next major optimization target.

## What we learned

1. **Measure before optimizing.** The TSC profiler cost us ~30 minutes to
   build and immediately identified the backtrace capture as the bottleneck
   — something we'd never have found by reading code.

2. **Debug instrumentation must be zero-cost when disabled.** Our `trace!()`
   macros, backtrace capture, and VGA output all ran unconditionally. Each
   was "just a few microseconds" but they compounded to 100x overhead.

3. **VM exits are the KVM tax.** Every `in`/`out` instruction, every MMIO
   write, every interrupt costs 1-10 µs. Linux kernels are carefully
   optimized to minimize these; we had them scattered everywhere.

4. **The profiler is permanent infrastructure.** Every future optimization
   can be validated with `KEVLAR_DEBUG="profile"` — we'll never again
   wonder "is this syscall slow?" without data.
