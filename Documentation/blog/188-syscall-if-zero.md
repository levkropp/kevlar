## Blog 188: syscalls run with IF=0, and that breaks every TLB shootdown

**Date:** 2026-04-19

[Blog 187](187-pcid-tlb-leak-fix.md) listed three hypotheses for why
task #25's kernel-pointer-leak corruption persists after the PCID
cross-flush fix:

1. Per-CPU PCID generation tracking gap
2. IF=0 transitions inside user-page free paths
3. Thread CPU migration between local flush and free

Hypothesis 1 was disproved this turn: implementing per-CPU
`LAST_SEEN_PCID_GEN` tracking made things slightly *worse* (3/10 runs
leaked, 12 hits vs the 1/10 baseline).  Reverted.

Hypothesis 2 is **confirmed** as the root cause, but the fix opens up
a chain of latent bugs that need their own audit before we can land it.

## Confirming hypothesis 2

Added a throttled diagnostic to `tlb_remote_flush_all_pcids`: on every
IF=0 entry (the path that skips the IPI and only bumps the global PCID
generation), log a marker, then on the first 8 hits dump a symbolicated
backtrace.

A 3-run xfce sample produced **6919, 10990, and 2950** IF=0 hits per
run.  The first 8 backtraces all looked identical:

```
TLB_FLUSH_IF0: tlb_remote_flush_all_pcids called with IF=0; backtrace:
    0: ffff8000002244d8  kevlar_platform::x64::apic::tlb_remote_flush_all_pcids()+0xe8
    1: ffff800000141d76  kevlar_kernel::syscalls::SyscallHandler::sys_munmap()+0x846
    2: ffff8000001527bc  kevlar_kernel::syscalls::SyscallHandler::do_dispatch()+0x54c
    3: ffff80000019d3db  kevlar_kernel::Handler::handle_syscall()+0x40b
    4: ffff80000022db48  x64_handle_syscall()+0x158
    5: ffff8000001013d1  syscall_entry()+0x47
```

Every IF=0 flush comes from `sys_munmap` (and presumably the other
user-page-free syscalls) called via `syscall_entry`.

## Why every syscall has IF=0

`platform/x64/syscall.rs` sets `MSR_FMASK = 0x0700`, which clears
RFLAGS bits 8 (TF), 9 (IF), and 10 (DF) on every `SYSCALL`
instruction.  Bit 9 is IF.  Clearing it is intentional — there's a
window between SYSCALL and SWAPGS where an interrupt would use the
*user's* GS base in kernel mode.

But `platform/x64/usermode.S::syscall_entry` does:

1. SWAPGS (closes the GS window — interrupts are now safe)
2. CLD
3. Switch to kernel stack
4. Push pt_regs
5. `call x64_handle_syscall`

…and there is **no STI anywhere**.  The entire syscall body runs with
IF=0.  Every `flush_tlb_remote` from a syscall hits the "interrupts
disabled, defer to bump" fallback path, which does nothing on remote
CPUs.  Stale TLB entries on those CPUs survive until they
context-switch — which can be seconds later, long enough for the
freed paddr to be recycled into kernel heap, slab, another user
process, etc.  The owner writes through the stale TLB and either
corrupts the new owner or reads back the new owner's data.  Blog
186/187's symptom — kernel direct-map pointers in user heaps — is
the user-side read of that read-back.

## Attempting the fix

### Attempt 1: STI in syscall_entry

Add one instruction in `usermode.S` right before `call
x64_handle_syscall`, after SWAPGS / stack switch / pt_regs build are
all done:

```asm
push rax     // syscall number
sti          // re-enable interrupts for the syscall body
call x64_handle_syscall
```

10-run xfce sample: **all 10 runs hung at Phase 5 (XFCE Session
startup)**.  Kernel boots, mounts rootfs, gets to the XFCE startup
phase, then qemu has to be killed at the timeout boundary.  No
panics, no SIGSEGVs — just no progress.

### Attempt 2: STI localized to the IPI broadcast

Less aggressive: enable IF only inside `tlb_remote_flush_all_pcids`
when the caller had IF=0, just to send the IPI and wait for ACK,
then restore IF=0:

```rust
let if_was_off = !interrupts_enabled();
if if_was_off { sti }
// ... lock, broadcast, spin-wait for ACK ...
if if_was_off { cli }
```

3-run sample: **also hung**.  NMI watchdog fired:

```
WATCHDOG: CPU 0 stuck! HB=1249 (unchanged for 4000ms). Sending NMI.
NMI WATCHDOG: CPU 0 STUCK
  RIP=0xffff80000018fd63  RSP=0xffff800030edee88
  RFLAGS=0x97  (IF=0, ring 0)
  preempt_count=1  need_resched=0
  Backtrace:
    #0: 0xffff80000012ce65
    #1: 0xffff8000001553d6
    #2: 0xffff80000019d3db   handle_syscall
    #3: 0xffff80000022da58   x64_handle_syscall
    #4: 0xffff8000001013d1   syscall_entry
  lockdep: no locks held
```

CPU 0 stuck inside a syscall, IF=0, lockdep clean.  The if-trace ring
buffer shows ~32 lock acquire/release events all with `→ IF=0`.  RIP
oscillates by 8 bytes between successive watchdog fires, so it's a
small spin loop.

So even briefly enabling interrupts during a TLB IPI exposes
*something* in the kernel that depends on IF=0 throughout a syscall.
Without identifying what that something is, we can't safely flip the
default.

## What this tells us

- **Root cause for task #25 is now identified**: syscalls running with
  IF=0 cause every cross-CPU TLB shootdown to silently degrade to
  "bump the generation, hope nobody dies."
- **A direct fix is not safe**: the kernel has accumulated code that
  implicitly assumes IF=0 across syscall boundaries.  Likely
  candidates: the scheduler (preempts during syscall would be new),
  per-CPU data accesses without preempt_disable, atomic update
  patterns that assumed atomicity by virtue of no-preemption.
- **Diagnostic is durable**: the IF=0 throttled logger and the
  KERNEL_PTR_LEAK marker remain in tree.  Anyone re-attempting this
  fix can see the IF=0 hit rate drop in real time.

The follow-up work (task #18) is to audit every IF=0-dependent
syscall path and either explicitly disable preemption / take the
right lock type, or refactor to not depend on IF=0.  That is a much
bigger investigation than a single-line `sti`.

## Three hypotheses, three outcomes

| hypothesis | status |
|---|---|
| 1: per-CPU PCID gen tracking gap | disproved (made things slightly worse) |
| 2: IF=0 in user-page free paths | confirmed (root cause) — fix blocked on broader audit |
| 3: thread migration between local flush and free | not investigated yet (lower priority — hypothesis 2 is the dominant case) |

The investigation has moved from "we don't know which of three" to
"we know it's hypothesis 2, and here's the precise mechanism, but
fixing it requires a multi-week audit of every IF=0-dependent
syscall path."  That is concrete progress, even though no production
code lands this turn.
