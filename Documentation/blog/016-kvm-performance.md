# From 13µs to 200ns: Closing the KVM Performance Gap

Our benchmarks showed getpid taking 13,000 ns per call on KVM — about 65x
slower than native Linux (~200 ns). Every other syscall was similarly
inflated. The kernel was functionally correct but unusably slow under
virtualization. Six targeted fixes brought getpid down to **200 ns**, with
2-5x improvements across the board.

## Why KVM is different

Under KVM, certain operations cause a "VM exit" — the CPU transitions from
guest mode back to the hypervisor, which handles the operation and returns.
Each VM exit costs 1-10 µs. On bare metal these operations are cheap; under
KVM they dominate.

The worst offenders:
- **Port I/O** (`in`/`out` instructions): serial UART, VGA cursor, PIT timer
- **MMIO writes**: APIC end-of-interrupt register
- **Spinlock cli/sti**: our SpinLock disables interrupts on acquire

## The six fixes

### 1. Remove serial TX busy-wait

```rust
// Before: busy-wait until transmit buffer ready
while (self.inb(LSR) & TX_READY) == 0 {}  // VM exit per poll
self.outb(THR, ch);                        // VM exit for write

// After: QEMU's virtual UART is always ready
self.outb(THR, ch);                        // single VM exit
```

QEMU's 16550 UART never reports "not ready" — the busy-wait was pure waste.
Each `inb` is a VM exit. For a single `info!()` message (~80 chars), that's
80 wasted VM exits eliminated.

### 2. Remove VGA output from serial printer

Every character printed to serial was *also* sent to the VGA text buffer via
`vga::printchar()`, which calls `move_cursor()` — **4 `outb()` calls** (4 VM
exits) per character just to move the blinking cursor. For 80 characters of
output, that's 320 VM exits eliminated. VGA is now only initialized at boot.

### 3. Remove interrupt handler trace logging

The interrupt handler had an unconditional `trace!()` that fired on every
non-timer interrupt, writing a formatted string to the serial port. Each
serial IRQ (keyboard input) triggered dozens of VM exits just for the debug
output. Tracing is now handled by the structured debug event system, which
only emits when `debug=irq` is explicitly enabled.

### 4. Reduce timer from 1000 Hz to 100 Hz

The PIT was configured for 1000 Hz — one timer interrupt per millisecond.
Each timer IRQ causes a VM exit for the interrupt delivery, plus an MMIO
write for the EOI. Reducing to 100 Hz cuts timer overhead by 10x.
Preemption interval stays at 30 ms (changed from 30 ticks to 3 ticks).

### 5. Bypass APIC spinlock for EOI

Every interrupt required `APIC.lock().write_eoi()` — our SpinLock disables
interrupts (`cli`), checks for deadlocks, acquires the lock, does the MMIO
write, releases the lock, and restores interrupts (`sti`). On a single-CPU
kernel with interrupts already disabled in the interrupt handler, this is
pure overhead. The fix inlines the EOI write directly:

```rust
let eoi_addr = PAddr::new(0xfee0_00b0).as_vaddr();
core::ptr::write_volatile(eoi_addr.as_mut_ptr::<u32>(), 0);
```

### 6. Lock-free signal check on syscall exit

Every syscall exit called `try_delivering_signal()`, which acquired a
spinlock on the signal delivery structure — even when no signals were
pending (the overwhelming common case). Added an `AtomicU32` mirror of
the pending signal bitmask, checked with a relaxed load before taking
the lock:

```rust
if current.signal_pending.load(Ordering::Relaxed) == 0 {
    return Ok(());
}
```

Also removed the unconditional `trace!()` from syscall dispatch that
formatted every syscall's arguments and wrote them to serial.

## Results

| Benchmark | Before | After | Speedup |
|-----------|--------|-------|---------|
| getpid | 13,000 ns | 200 ns | **65x** |
| read_null | 26,000 ns | 14,000 ns | 1.9x |
| write_null | 28,000 ns | 14,000 ns | 2.0x |
| pipe | 625,000 ns | 312,500 ns | 2.0x |
| stat | 264,000 ns | 48,000 ns | 5.5x |
| open_close | 95,000 ns | 70,000 ns | 1.4x |

getpid at 200 ns is Linux-class syscall latency. The remaining gaps in
read/write/stat are in the VFS path — lock acquisitions in the opened file
table, path lookup, and usercopy overhead for small buffers. These are the
next optimization targets.

## What's next

The timer-based `clock_gettime(CLOCK_MONOTONIC)` has 10 ms granularity at
100 Hz, making sub-millisecond benchmarks unreliable. Wiring `rdtsc` into
the clock subsystem would give nanosecond resolution. Per-syscall cycle
counters, VM exit profiling, and lock contention tracking are the
infrastructure needed to close the remaining gaps.
