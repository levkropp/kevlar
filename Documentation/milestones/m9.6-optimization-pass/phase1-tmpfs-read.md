# M9.6 Phase 1: tmpfs Read Path

**Regressions:** `pipe_grep` (15.1x), `sed_pipeline` (21.4x)
**Target:** Both within 1.1x of Linux KVM

## The problem

`pipe_grep` runs `sh -c "grep apple /tmp/bench_grep > /dev/null"` in a
fork+exec loop.  The input file is a 3.6KB tmpfs file (100 repetitions
of 3 lines).  On Linux KVM this takes 65µs per iteration.  On Kevlar
it takes 979µs — 15x slower.

`sed_pipeline` runs `sh -c "sed 's/.../.../' /tmp/bench_sed > /dev/null"`
on a ~5KB file.  Linux: 64µs.  Kevlar: 1.37ms — 21x slower.

Both benchmarks do fork + exec sh + exec grep/sed + read tmpfs file +
write /dev/null + exit.  The exec overhead itself is ~177µs (from
`exec_true`), so the remaining 800µs-1.2ms is in file I/O and
process teardown.

## Analysis approach

1. **Profile with per-syscall cycle counter** — rebuild with
   `KEVLAR_DEBUG=profile` and run each benchmark to identify which
   syscalls dominate wall time

2. **Isolate tmpfs read** — write a micro-benchmark that does
   `open + read(3.6KB) + close` in a loop on tmpfs (no fork/exec),
   compare to Linux

3. **Check lock contention** — tmpfs `read()` holds `lock_no_irq()`
   across the entire usercopy:
   ```rust
   fn read(&self, offset: usize, buf: UserBufferMut<'_>, ...) -> Result<usize> {
       let data = self.data.lock_no_irq();  // IRQs disabled here
       // ...
       writer.write_bytes(&data[offset..])  // usercopy while IRQs off
   }
   ```
   On a single CPU with no contention, this shouldn't matter.  But
   disabled IRQs delay the timer tick → no preemption → child process
   may not get scheduled promptly after parent calls `waitpid`.

4. **Check /dev/null write** — `write_null` benchmark shows 1.2x
   regression (158ns vs 132ns).  grep/sed do many small writes to
   /dev/null — the 26ns overhead per write accumulates.

## Potential fixes

### Fix A: tmpfs lock_no_irq → lock

Replace `self.data.lock_no_irq()` with `self.data.lock()` in both
`read()` and `write()`.  `lock_no_irq` was used for performance (skips
cli/sti), but IRQ disabling during multi-KB usercopies delays timer
ticks and can stall the scheduler.  The tmpfs data lock is never
accessed from IRQ context, so regular `lock()` is safe.

### Fix B: /dev/null fast path

`/dev/null` read returns 0, write returns `buf.len()`.  Check if
there's unnecessary fd table locking or file offset tracking on the
fast path that can be skipped.

### Fix C: execve demand-paging overhead

The fork+exec pattern demand-pages the new process's text and data
segments.  Each page fault takes ~2µs (our `mmap_fault` benchmark).
BusyBox is ~1.4MB with ~350 4KB pages.  If we fault in 50-100 pages
per exec, that's 100-200µs of page fault overhead alone.  This is
measured in Phase 2 but may overlap here.

## Success criteria

- `pipe_grep` < 72µs (within 1.1x of Linux's 65µs)
- `sed_pipeline` < 70µs (within 1.1x of Linux's 64µs)
- `read_null` < 145ns (within 1.1x of Linux's 132ns)
- `write_null` < 145ns (within 1.1x of Linux's 132ns)
