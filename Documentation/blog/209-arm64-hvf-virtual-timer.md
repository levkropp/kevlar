## Blog 209: Kevlar ARM64 boots under HVF — one register swap

**Date:** 2026-04-23

Blog 208 closed with 159/159 arm64 contract parity under TCG and a
TODO to get Kevlar running on Apple Silicon's hypervisor.  Under
`-accel hvf -cpu host` Kevlar was panicking early with the cryptic
message

```
unhandled synchronous exception: ec=0x0, esr=0x2000000,
pc=0xffff0000401fc8dc, far=0x0
```

Decoded: EC=0 is "Unknown reason for exception" — canonical ARMv8
code for an undefined / disabled instruction.  far=0 rules out a
memory fault.  pc is kernel text early in boot.

## Finding the instruction

The symbol table was empty (`rustfilt` not installed, NM step
produced a 0-byte file), so the usual `addr2line`-style resolve
didn't work.  Direct disassembly with
`llvm-objdump --start-address=0xffff0000401fc880` did:

```
ffff0000401fc8a8 <kevlar_platform::arm64::timer::init>:
  ...
  ffff0000401fc8dc:  d51be208    msr   CNTP_TVAL_EL0, x8
```

The faulting instruction was writing to the **physical** timer's
tval register in `timer::init`.  That fit the symptom exactly: EL=0
"unknown" traps are what you get when the hypervisor refuses a
register access it doesn't mediate.

## Why HVF traps CNTP

Apple's Hypervisor.framework doesn't give EL1 guests access to the
physical timer at all — CNTP_CTL_EL0, CNTP_TVAL_EL0, CNTPCT_EL0 are
all trapped as UNDEFINED.  Only the hypervisor itself drives the
physical clock; guests get the **virtual** timer (CNTV_\*) plus
CNTVOFF_EL2 handling for free.

This is a widely-shared pattern across arm64 hypervisors.
KVM-on-Linux allows CNTP but the standard Linux arm64 timer driver
still prefers CNTV, because CNTV is the only thing that works
everywhere (HVF, Xen, nested virt, cloud, bare-metal).  Kevlar's
original port picked CNTP presumably because it was the first thing
in the ARM ARM's table.  Wrong choice for the real world.

## The fix

```diff
-//! Uses CNTP (physical timer) at EL1.
-//! Timer IRQ = PPI 14 -> GIC IRQ 30.
+//! Uses CNTV (virtual timer) at EL1.  Apple's Hypervisor.framework
+//! traps every CNTP_* access as UNDEFINED for EL1 guests ...
+//! Timer IRQ = PPI 11 → GIC IRQ 27.
-pub const TIMER_IRQ: u8 = 30;
+pub const TIMER_IRQ: u8 = 27;
...
-    asm!("msr cntp_tval_el0, {}", in(reg) tval);
-    asm!("msr cntp_ctl_el0,  {}", in(reg) 1u64);
+    asm!("msr cntv_tval_el0, {}", in(reg) tval);
+    asm!("msr cntv_ctl_el0,  {}", in(reg) 1u64);
...
-    unsafe { asm!("mrs {}, cntpct_el0", out(reg) val) };
+    unsafe { asm!("mrs {}, cntvct_el0", out(reg) val) };
```

Six mnemonics, one IRQ constant.  CNTFRQ_EL0 stays as-is — it's
shared between the two timers.

## Result

- HVF boot: reaches `~ #` shell prompt with timer firing, scheduler
  running.
- Contract suite under HVF: 159/159 (matches TCG).

Added a `--accel tcg/kvm/hvf` flag to `tools/compare-contracts.py`
so the same harness runs under either backend; HVF also flips the
default `-cpu cortex-a72` to `-cpu host` since HVF can't emulate a
different ARM model.

## Benchmarks: Kevlar HVF vs Linux HVF

Same benchmark binary (aarch64-linux-musl) in the same initramfs,
booted on Kevlar and on Alpine's arm64 kernel, both under HVF.
`bench.c`'s `--quick` profile, 47 comparable benchmarks:

| Category | Count | Typical ratio |
|----------|-------|---------------|
| Kevlar faster | 33 | 0.3–0.7× (2–3× speedup on simple syscalls) |
| within 10% | 5 | |
| within 30% | 7 | |
| Kevlar slower | 14 | mostly fork/exec derivatives |

The faster column is mostly trivial syscalls: getpid, read_null,
write_null, mmap_munmap, mprotect, sched_yield, poll — each in the
60–160 ns range for Kevlar vs 130–230 ns for Linux.  Thin kernel
with no seccomp / audit / cgroup / namespace overhead.

The slow outliers are all descendants of the same problem:

```
fork_exit       165 831 ns   vs Linux 16 195 ns   10.24× slower
exec_true       154 972 ns   vs       31 293 ns    4.95× slower
shell_noop      188 586 ns   vs       47 223 ns    3.99× slower
socketpair        7 569 ns   vs        1 503 ns    5.04× slower
```

Fork is the root.  Kevlar's arm64 ghost-fork path falls back to
plain `duplicate_table` (eager CoW-less PT copy) — that's documented
in blog 207 as a known stub.  Every benchmark that forks (directly
or via `system` / shell) pays the full eager-copy cost.  Until the
arm64 CoW path lands, the 10× gap is the ceiling.

Other flagged slowdowns (pipe 3.4×, read_zero 3.4×, mmap_fault
3.2×) aren't fork-derived and look like genuine hot-path
regressions worth their own investigations.

## Next

Port arm64 ghost-fork with real CoW.  Estimated impact: pulls
`fork_exit` from 10.24× slower to ~1.5× (Linux ghost-fork is
hand-optimized with years of tuning we don't need to match on
day one), closes the derivative gaps in `exec_true` and
`shell_noop`, and pushes Kevlar-arm64 into a posture where "within
10% of Linux on most syscalls" is a realistic target.
