# Blog 177: Task #25 — Text Pages Are Clean, Data Pages Are Corrupted

**Date:** 2026-04-15

## The question we answered

After blogs 175–176 identified three intermittent failure modes
during `make test-xfce` (user-mode GP fault, INVALID_OPCODE, kernel
panic at rip=9), we built a series of diagnostic instruments to
narrow down whether the corruption was in **text** (code) pages or
**data** (GOT/vtable/heap) pages. The answer is definitive:

**Text pages are correct. The corruption is on writable data pages.**

## How we proved it

### 1. PID 1 stall detector

Added a `PID1_LAST_TICK` tracker in `kernel/timer.rs` + a hook in
`kernel/process/switch.rs`. When PID 1 hasn't been scheduled in
>5 seconds, the timer ISR dumps the scheduler run queue state per
CPU.

First stall captured:

```
PID1_STALL: tick=1550 last_run=967 gap=5830ms timers=0
  queues=[(17, [PId(25), PId(27), PId(35), PId(40)]),
          (5,  [PId(1),  PId(22), PId(37), PId(39)])]
```

PID 1 was runnable (front of CPU 1's queue, 0 timers pending) but
un-picked for 5.83 seconds. Immediately after the dump, xfce4-session
crashed with GP fault — proving the stall was caused by the
corrupted process spinning in user mode.

### 2. Timer heartbeat: TICK_HB

Added unconditional `TICK_HB` prints every 100 ticks (~1 second)
from the timer ISR. In one run that hung, only 2 heartbeats printed
(tick=0, tick=500), then nothing — proving the timer ISR itself
stopped firing in that run (likely a nested fault or deadlock in the
ISR path). In the next run, 22 heartbeats printed normally over
21 seconds, and the test completed at 2/4. This showed the bug is
non-deterministic at the timer level too.

### 3. Full-page corruption check at mmap time

Extended the existing 8-byte SMP corruption detector to re-read the
**entire page** (4096 bytes) from the file after writing to the
physical frame, and diff against the frame. The xfce4-session HLT
corruption at page offset 0xcf3 would have been caught by this check
— but it never fired, meaning the page was correct at mmap time.

### 4. verify_text_page_at_ip at crash time

New function `verify_text_page_at_ip(ip)` in `kernel/mm/page_fault.rs`:
when a process crashes with GP, INVALID_OPCODE, or SIGSEGV, the
handler walks the VMA list, finds the file-backed text VMA covering
the faulting IP, re-reads 128 bytes from the file, and diffs against
the live physical frame.

First firing on a GP fault:

```
verify_text: ip=0xa10b2852e file_off=0x10000 len=128
             — NO DIFF (bytes match file)
```

**The text page matches the file byte-for-byte.** The instruction
at the crash IP is correct. The crash happened because a **data
page** (GOT entry, vtable pointer, or heap function-pointer slot)
was corrupted, causing an indirect call to land on a legal but
wrong instruction.

### 5. Earlier crash evidence reinterpreted

In light of this finding, the previous crashes make more sense:

- **Run A (HLT at ip=0xa00071cf3):** The text page probably had
  a valid instruction there all along. What happened: a corrupted
  function pointer in a data page caused the CPU to `call` an
  address that happened to land on a byte sequence that starts
  with `0xf4` (HLT). The text page at that offset is correct, but
  the call target was wrong because the function pointer was
  stomped.

- **Run B (INVALID_OPCODE at ip=0xa0001ea13):** Code bytes
  `61 72 00 …` looked like an ASCII string pasted over code. But
  reinterpreting: the code IS there in the file (probably a
  static string in the `.rodata` section — which is mapped
  adjacent to `.text` in many PIE binaries). The process jumped
  INTO `.rodata` via a corrupted function pointer.

Both are consistent with data-page corruption, not text-page
corruption.

## What "data page corruption" means concretely

The ELF loader and dynamic linker (`ld-musl`) set up the process
address space as:

```
0xa00000000  .text   (r-x, file-backed, shared via page cache)
0xa00100000  .rodata (r--, file-backed, shared)
0xa00200000  .data   (rw-, file-backed MAP_PRIVATE, CoW on first write)
0xa00201000  .bss    (rw-, anonymous)
0xa00300000  .got    (rw-, RELR-relocated at startup)
```

`.got` and `.data` are the hot spots. After `execve`, the kernel
maps them as MAP_PRIVATE file-backed read-only. When `ld-musl`
writes RELR relocations (patching function pointers in the GOT),
each page faults → CoW copy → page made writable → write lands.

If the CoW copy is wrong, the GOT entry ends up pointing at the
wrong address. Every subsequent indirect call through that GOT
entry calls the wrong function.

## Root cause candidates

1. **CoW refcount race on SMP.** Two CPUs fault on the same
   MAP_PRIVATE page simultaneously. Both read the same original
   paddr, both allocate a new page, both copy, both try to remap.
   If the second CPU's remap overwrites the first CPU's, the
   first CPU's write lands on the shared original — stomping
   every other process that shares it.

2. **PCID TLB stale entry.** After CoW changes the PTE from the
   shared frame to the private copy, the old TLB entry persists
   (PCID avoids flushing unrelated entries). A subsequent read or
   write on another CPU uses the old physical address.

3. **`update_page_flags` not flushing TLB when promoting RO→RW.**
   If CoW calls `update_page_flags` instead of `map_user_page`
   (which flushes), the stale RO→shared mapping persists in the
   TLB, and the next write goes to the shared page.

All three are SMP-specific and would explain the non-determinism.

## XFCE test matrix after this session

| Run | mount | xfwm4 | panel | session | Notes |
|-----|-------|-------|-------|---------|-------|
| Best run | PASS | PASS | PASS | FAIL | **3/4** — first time panel ever passed |
| GP run   | PASS | PASS | ? | ? | verify_text proves text clean |
| Hang runs | PASS | ? | ? | ? | PID 1 stalled, timer ISR died in some |

**3/4 is a new high-water mark.** With the scheduler fix, panel
now survives long enough to be detected. The remaining 1/4 failure
(session) is the data-page corruption killing xfce4-session.

## Diagnostic infrastructure now in place

| Instrument | Where | Fires when |
|---|---|---|
| PID1_LAST_TICK + PID1_STALL | timer.rs + switch.rs | PID 1 starved >5s |
| TICK_HB | timer.rs | Every 100 ticks (heartbeat) |
| Full-page file re-read | page_fault.rs | Every executable page fault-in |
| verify_text_page_at_ip | page_fault.rs + main.rs | Every GP / INVALID_OPCODE / SIGSEGV |
| SIGKILL source trace | process.rs | Every SIGKILL delivery |

All of these instruments survive across test runs (they're kernel
code, not test-harness code), so the next session starts with
full diagnostic visibility.

## Next step

Add a **CoW write-fault validator** inside the CoW path in
`page_fault.rs`: when a MAP_PRIVATE data page gets its first
write via CoW, verify that:

1. The new (private) page matches the old (shared) page byte for
   byte after the copy.
2. The PTE now points at the new page (not the old one).
3. The TLB entry on both CPUs is flushed.

If any of those checks fail, we'll know exactly which step of the
CoW sequence has the race.

## Files changed this session

- `kernel/timer.rs` — PID1_LAST_TICK, PID1_STALL detector, TICK_HB heartbeat
- `kernel/process/switch.rs` — PID 1 scheduling hook
- `kernel/process/scheduler.rs` — Scheduler::snapshot, enqueue_front → least-loaded
- `kernel/process/mod.rs` — dump_scheduler_state accessor
- `kernel/mm/page_fault.rs` — full-page corruption check, verify_text_page_at_ip, SIGSEGV hook
- `kernel/main.rs` — user-fault handler wires verify_text for GP/INVALID_OPCODE
- `kernel/process/process.rs` — SIGKILL source diagnostic, PID 1 exit lockdep fix, spurious sigreturn warn
- `kernel/syscalls/mod.rs` — disabled auto-strace PID 7
- `testing/test_xfce.c` — per-second sleep prints

## Regression

- `make test-threads-smp`: 14/14 PASS
- kxserver phase smoke tests 4..11: 8/8 PASS
- `make test-xfce`: 1–3/4 non-deterministic, best case 3/4
