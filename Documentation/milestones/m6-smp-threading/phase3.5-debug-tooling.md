# Phase 3.5: SMP Debug Tooling

**Goal:** Build the visibility infrastructure needed to diagnose SMP crashes
before continuing with Phase 4 (TLB shootdown / thread safety). This phase
was inserted after three days of analytic deadlock on a `rip=0x87` kernel
page fault on CPU=1 that the existing tooling could not explain.

---

## Why This Phase Exists

After completing Phase 3 (threading, 12/12 mini_threads pass) we began
implementing TLB shootdowns (Phase 4). The work introduced a regression:
the test suite crashed with a kernel page fault at `rip=0x87` on CPU=1 before
any test case ran. Three days of analysis produced no root cause because:

1. **The backtrace falls apart at interrupt frame boundaries.** The unwinder
   walks RBP chains. At the interrupt frame (pushed by the CPU hardware),
   there is no RBP linkage. The unwinder stumbles onto garbage and reports
   `GS_RSP0+0x4` as the caller — completely uninformative.

2. **We don't see what CPU=1 was doing when interrupted.** We know the fault
   is at `rip=0x87`, but we don't know whether CPU=1 was in the idle loop,
   inside a TLB shootdown handler, or in a context switch. That single fact
   would immediately narrow the cause.

3. **No cross-CPU state at crash time.** The panic prints only the faulting
   CPU's registers. The other CPUs' recent activity is invisible.

4. **The crash dump is broken.** `boot2dump` tries to re-probe virtio block
   devices from a cold-boot context and fails (`failed to find a virtio block
   device`), so no crash dump is ever written.

5. **Page poison exists but only in `debug_assertions` builds.** The value
   `0x87` is suspicious (small, non-aligned, looks like a single-byte field
   reinterpreted as a pointer). We cannot rule out use-after-free without
   poison being consistently present.

---

## What Already Exists

| Feature | Status | Location |
|---------|--------|----------|
| `broadcast_halt_ipi()` on panic entry | ✅ Done | `kernel/lang_items.rs:69` |
| Double-panic guard (`PANICKED` atomic) | ✅ Done | `kernel/lang_items.rs:58` |
| Structured JSONL debug events | ✅ Done | `kernel/debug/` |
| RBP-chain backtrace + symbol table | ✅ Done | `platform/backtrace.rs` |
| Page poison on free (`0xa5`) | ✅ Done (debug builds only) | `platform/page_allocator.rs:294` |
| Crash dump via boot2dump | ❌ Broken | `kernel/lang_items.rs:194` |
| Interrupted-context symbol on kernel panic | ❌ Missing | — |
| Per-CPU flight recorder | ❌ Missing | — |

---

## Improvements to Implement

### Improvement 1 — Better Interrupt Frame Context in Kernel Panics

**Problem:** When a kernel page fault fires (e.g., at `rip=0x87`), the panic
message prints the faulting RIP and RSP but no symbol name, no CS decode, and
no backtrace from the *interrupted* context.

**Root cause of missing info:** The `panic!` macro captures a backtrace from
the *panic handler's own frame*, which is deep inside `x64_handle_interrupt`
→ `rust_begin_unwind`. The interrupted code's frame is not in that chain.

**What to add in `platform/x64/interrupt.rs`:**

At the kernel page fault panic site (lines 202–211), before calling `panic!`:

1. **Decode CS.RPL** — `frame.cs & 3`: if 0 = kernel→kernel fault (rare,
   serious), if 3 = should have been classified as user fault (bug in
   `occurred_in_user` logic).

2. **Resolve symbol for `frame.rip`** — call `kevlar_platform::backtrace::
   resolve_symbol(VAddr::new(frame.rip as usize))` to print something like:
   `interrupted at: ffff800000101234 idle()+0x12`.

3. **Walk the interrupted stack** — use `frame.rbp` as the starting RBP and
   do an RBP-chain walk up to 8 frames. This gives a backtrace of what was
   running *when the fault occurred*, not the panic handler's own call chain.

4. **Print full register state** — dump all general-purpose registers from
   `InterruptFrame` as a compact one-liner. Especially useful for catching
   wild pointer values in rax/rbx/etc.

Apply the same improvement to the generic `kernel exception` panic path (line
154) so all kernel-mode faults get this treatment.

**Expected output improvement:**
```
[PANIC] CPU=1 at platform/x64/interrupt.rs:205
page fault occurred in the kernel: rip=87, rsp=ffff800001dae808, vaddr=87
  interrupted: CS=0x08 (ring 0), RFLAGS=0x202, RSP=ffff800001dae800
  interrupted at: 0000000000000087 (no symbol — below kernel base)
  interrupted backtrace (from rbp=0):
    [no valid frames — rbp=0 or corrupt]
  registers: rax=0 rbx=0 rcx=0 rdx=0 rsi=0 rdi=0
             r8=0 r9=0 r10=0 r11=0 r12=0 r13=0 r14=0 r15=0
```
Even "no symbol, below kernel base" is vastly more informative than the
current output — it immediately tells us the interrupted RIP is *not* a
kernel address at all, pointing to a corrupted return address or wild jump.

**Files:** `platform/x64/interrupt.rs`

#### Implementation Plan

**Key architectural fact:** `platform/x64/interrupt.rs` is inside the
`kevlar_platform` crate, same as `platform/backtrace.rs`. No public API
changes are required — `pub(crate)` visibility is sufficient throughout.

**Change 1 — `platform/x64/backtrace.rs`**

Add a second constructor that starts the RBP chain from an arbitrary address
instead of the current CPU register:

```rust
pub fn from_rbp(rbp: u64) -> Backtrace {
    Backtrace { frame: rbp as *const StackFrame }
}
```

`traverse` already handles null and out-of-range RBPs via `frame.is_null()`
and `VAddr::is_accessible_from_kernel`. No other changes needed to the
traversal logic.

**Change 2 — `platform/backtrace.rs`**

Add one `pub(crate)` function that uses both `resolve_symbol` (already
private to this module) and the new `Backtrace::from_rbp`:

```rust
/// Print the symbol and call chain of the code that was interrupted
/// by a hardware exception. Called from kernel fault handlers before
/// they call panic!, so the output appears before the panic backtrace.
pub(crate) fn print_interrupted_context(rip: u64, rbp: u64) {
    let vaddr = VAddr::new(rip as usize);
    if let Some(sym) = resolve_symbol(vaddr) {
        warn!("  interrupted at: {:016x}  {}+{:#x}",
            rip, sym.name, rip as usize - sym.addr.value());
    } else if rip < 0xffff_8000_0000_0000 {
        warn!("  interrupted at: {:016x}  \
               (below kernel base — not a kernel address)", rip);
    } else {
        warn!("  interrupted at: {:016x}  (no symbol)", rip);
    }

    warn!("  interrupted backtrace (from rbp={:#x}):", rbp);
    Backtrace::from_rbp(rbp).traverse(|i, vaddr| {
        if let Some(sym) = resolve_symbol(vaddr) {
            warn!("    {}: {:016x}  {}+{:#x}",
                i, vaddr.value(), sym.name, vaddr.value() - sym.addr.value());
        } else {
            warn!("    {}: {:016x}  (unknown)", i, vaddr.value());
        }
    });
}
```

The `rip < 0xffff_8000_0000_0000` check is the critical diagnostic branch:
it immediately tells us whether the fault address is in kernel space or not.
For `rip=0x87` this prints "below kernel base — not a kernel address", which
is the most informative single line of output we could produce for that crash.

**Change 3 — `platform/x64/interrupt.rs`**

The `InterruptFrame` struct is `#[repr(C, packed)]`. All field accesses must
copy the field to a local variable first to avoid unaligned references (the
existing code already does this for `rip` and `rsp`). Add locals for all
fields used in the new output.

At the kernel page fault site (currently line 202), before the `panic!`:

```rust
if !occurred_in_user {
    // Copy all packed fields before use.
    let rip    = frame.rip;
    let rsp    = frame.rsp;
    let rbp    = frame.rbp;
    let cs     = frame.cs;
    let rflags = frame.rflags;
    let rax    = frame.rax; let rbx = frame.rbx;
    let rcx    = frame.rcx; let rdx = frame.rdx;
    let rsi    = frame.rsi; let rdi = frame.rdi;
    let r8     = frame.r8;  let r9  = frame.r9;
    let r10    = frame.r10; let r11 = frame.r11;
    let r12    = frame.r12; let r13 = frame.r13;
    let r14    = frame.r14; let r15 = frame.r15;

    warn!("[kernel fault context]");
    warn!("  interrupted: ring={}, CS={:#x}, RFLAGS={:#018x}",
          cs & 3, cs, rflags);
    warn!("  regs: rax={:016x} rbx={:016x} rcx={:016x} rdx={:016x}",
          rax, rbx, rcx, rdx);
    warn!("        rsi={:016x} rdi={:016x} rbp={:016x} rsp={:016x}",
          rsi, rdi, rbp, rsp);
    warn!("        r8 ={:016x} r9 ={:016x} r10={:016x} r11={:016x}",
          r8, r9, r10, r11);
    warn!("        r12={:016x} r13={:016x} r14={:016x} r15={:016x}",
          r12, r13, r14, r15);
    crate::backtrace::print_interrupted_context(rip, rbp);

    panic!(
        "page fault occurred in the kernel: rip={:x}, rsp={:x}, vaddr={:x}",
        rip, rsp, cr2()
    );
}
```

Apply the same `warn!` block + `print_interrupted_context` call to the
`kernel exception` branch (currently line 154) so all kernel-mode faults
get this treatment.

#### Edge Cases

| Situation | Behaviour |
|-----------|-----------|
| `rbp = 0` | `from_rbp(0)` → null pointer → `traverse` bails immediately on `frame.is_null()` check; backtrace section prints nothing |
| `rbp` is a user-space address | `VAddr::is_accessible_from_kernel` returns false → `traverse` stops immediately |
| `rip` below kernel base | Printed as "below kernel base — not a kernel address" |
| Symbol table not populated | `resolve_symbol` returns `None` → printed as "no symbol" |
| Packed struct fields | All fields copied to locals before use |

#### Expected Output for the `rip=0x87` Crash

```
[kernel fault context]
  interrupted: ring=0, CS=0x8, RFLAGS=0x000000000000000202
  regs: rax=0000000000000000 rbx=0000000000000000 rcx=0000000000000000 rdx=0000000000000000
        rsi=0000000000000000 rdi=0000000000000042 rbp=0000000000000000 rsp=ffff800001dae808
        r8 =0000000000000000 r9 =0000000000000000 r10=0000000000000000 r11=0000000000000000
        r12=0000000000000000 r13=0000000000000000 r14=0000000000000000 r15=0000000000000000
  interrupted at: 0000000000000087  (below kernel base — not a kernel address)
  interrupted backtrace (from rbp=0x0):
    [empty — rbp is null or not accessible]
[PANIC] CPU=1 at platform/x64/interrupt.rs:205
page fault occurred in the kernel: rip=87, rsp=ffff800001dae808, vaddr=87
```

The `rbp=0` and "below kernel base" combination immediately tells us:
- The interrupted code had no valid call stack
- The instruction pointer was not in kernel space
- This is not an accidental page in the wrong place — something deliberately
  (or accidentally through corruption) put 0x87 into RIP

The register dump (especially `rdi=0x42` in the example above, which is
`TLB_SHOOTDOWN_VECTOR`) could reveal which code path led to the bad RIP,
even when the backtrace itself is empty.

---

### Improvement 2 — Page Poison Always On

**Problem:** Freed pages are only poisoned with `0xa5` in `debug_assertions`
builds. The performance profile (`PROFILE=performance`) uses Cargo's dev
profile which *should* have `debug_assertions = true`, but the behavior is
profile-dependent. We need poison unconditionally for SMP debugging.

**Analysis of `0x87`:** The value 0x87 = 135 = 0b10000111. If a freed page
were poisoned with `0xa5` (165 = 0b10100101), we would not see 0x87 from
page poison. However, if the kernel stack itself was freed and the memory
reused (a use-after-free of a kernel stack), other patterns could appear.
The value 0x87 does not match any obvious poison or fill pattern, which is
informative: it suggests a specific small integer being misread as a code
pointer (e.g., a `u8` field, a vector number, or a segment offset).

**What to change in `platform/page_allocator.rs`:**

Remove the `if cfg!(debug_assertions)` guard around the `write_bytes(0xa5)`
call in `free_pages`. Always poison. The 0xa5 fill is cheap (a few hundred
ns per page) compared to the cost of debugging a use-after-free for days.

Add a compile-time note: the poison can be disabled with a future
`feature = "no-poison"` flag when production performance matters.

**Files:** `platform/page_allocator.rs`

---

### Improvement 3 — Per-CPU Flight Recorder

**Problem:** We have no record of what each CPU was doing in the moments
before a crash. For SMP bugs this is the single most important piece of
missing information.

**Design:**

A **lock-free per-CPU ring buffer** with 64 entries per CPU, 8 CPUs max.
Each entry is 32 bytes (4 × u64). Entries are written by the CPU that owns
the slot — no cross-CPU synchronization needed during writes. On panic, all
other CPUs have been halted, so the panic handler can read all buffers safely.

```
Entry layout (32 bytes):
  [0] tsc:u64       — TSC timestamp
  [1] kind:u8 | cpu:u8 | _pad:u16 | data0:u32   — packed into u64
  [2] data1:u64
  [3] data2:u64
```

**Event kinds:**

| Kind | Name | data0 | data1 | data2 |
|------|------|-------|-------|-------|
| 1 | `CTX_SWITCH` | from_pid | to_pid | — |
| 2 | `TLB_SEND` | target_mask | vaddr | num_pages |
| 3 | `TLB_RECV` | vaddr (0=full) | cpu_sender | — |
| 4 | `MUNMAP` | pid | addr | len |
| 5 | `MMAP_FAULT` | pid | fault_addr | — |
| 6 | `PREEMPT` | pid | — | — |
| 7 | `SYSCALL_IN` | nr | arg0 | — |
| 8 | `SYSCALL_OUT` | nr | ret as u64 | — |
| 9 | `LOCK_WAIT` | lock_id | holder_cpu | — |
| 10 | `IDLE_ENTER` | cpu | — | — |
| 11 | `IDLE_EXIT` | cpu | vec | — |

**Write path (called from hot paths):**

```rust
pub fn fr_record(kind: u8, data0: u32, data1: u64, data2: u64) {
    let cpu = crate::arch::cpu_id() as usize;
    let idx = FR_IDX[cpu].fetch_add(1, Relaxed) % RING_SIZE;
    // Non-atomic multi-word write: only this CPU writes to this slot.
    let slot = &mut FR_RINGS[cpu][idx];
    slot[0] = tsc::read_raw();
    slot[1] = ((kind as u64) << 56) | ((cpu as u64) << 48) | (data0 as u64);
    slot[2] = data1;
    slot[3] = data2;
}
```

**Read/dump path (called from panic handler, all other CPUs halted):**

Reads all 8 × 64 entries, sorts by TSC, prints as a unified timeline.

```
[FLIGHT RECORDER — last 128 events across all CPUs]
  TSC+0000us CPU=0 SYSCALL_IN    nr=11 (munmap) arg0=0xa0000000
  TSC+0001us CPU=1 IDLE_EXIT     vec=0x42 (TLB_SHOOTDOWN)
  TSC+0001us CPU=1 TLB_RECV      vaddr=0 (full flush)
  TSC+0002us CPU=0 TLB_SEND      mask=0xe vaddr=0 pages=512
  TSC+0003us CPU=1 IDLE_ENTER    cpu=1
  TSC+0005us CPU=0 SYSCALL_OUT   nr=11 ret=0
```

This output would immediately show whether CPU=1 received a TLB shootdown,
what vector was involved, and what it was doing before and after.

**Integration points** (where `fr_record` calls are added):
- `kernel/process/switch.rs` — CTX_SWITCH at the top of `switch()`
- `platform/x64/apic.rs` — TLB_SEND in `tlb_shootdown` and `tlb_remote_full_flush`
- `platform/x64/interrupt.rs` — TLB_RECV in TLB_SHOOTDOWN_VECTOR handler; IDLE_EXIT/ENTER around hlt
- `kernel/syscalls/munmap.rs` — MUNMAP at syscall entry/exit
- `kernel/timer.rs` — PREEMPT when timer fires and switch() is called
- `kernel/syscalls/mod.rs` or individual syscall handlers — SYSCALL_IN/OUT for key syscalls

**Files:** New `platform/flight_recorder.rs` + integration in multiple call sites.

---

### Improvement 4 — Fix the Crash Dump

**Problem:** `boot2dump` calls `save_to_file_and_reboot` which re-probes
virtio block devices from scratch. In the crash context (other CPUs halted,
virtio driver possibly in a broken state from the crash), this probe fails
100% of the time.

**Diagnosis:**
```
[boot2dump] PANIC: failed to find a virtio block device
```
`boot2dump` is a second-stage bootloader that runs *after* rebooting into a
minimal environment to save the crash dump. The reboot should give it a clean
virtio device. The failure suggests either:
- The crash dump magic (`0xdeadbeee`) is not being preserved across the reboot
  in QEMU's RAM (QEMU does not preserve RAM across `-reset` by default)
- The boot2dump image cannot find the block device because the virtio device
  is not enumerated in the minimal environment it boots into

**Fix approach — serial-only crash dump:**

Instead of booting a second stage, write the crash dump directly to the QEMU
serial port in a delimited format that `run-qemu.py` can capture:

```
<<<KEVLAR_CRASH_DUMP_BEGIN>>>
<base64-encoded dump data>
<<<KEVLAR_CRASH_DUMP_END>>>
```

`run-qemu.py` watches for this sentinel on the serial port and writes
`build/kevlar.dump` automatically. This requires no reboot, no virtio, no
second stage, and works in all environments (TCG, KVM, real hardware with a
serial cable).

The existing `KernelDump` struct + boot2dump path can remain as the
*secondary* dump path for environments that support it. The serial path is
the new primary.

**Files:**
- `kernel/lang_items.rs` — add serial dump output after current crash dump
- `testing/run-qemu.py` — add sentinel detection and dump extraction
- Potentially add a `base64` encoder to `platform/` (simple lookup-table
  implementation, no std dependency)

---

## Implementation Order

Each improvement is independent. The order is:

| # | Improvement | Effort | Value |
|---|-------------|--------|-------|
| 1 | Better interrupt frame context | ~2h | Immediate: shows what was running when interrupted |
| 2 | Page poison always on | ~15m | Immediate: rules out use-after-free as root cause |
| 3 | Per-CPU flight recorder | ~4h | Highest: full timeline of cross-CPU activity |
| 4 | Fix crash dump (serial) | ~2h | Medium: needed for headless/CI debugging |

Start with 1 and 2 as they are quick and give immediate signal on re-running
the failing test. Then 3, then 4.

---

## What We Expect to Learn

When the `test-threads-smp PROFILE=performance` test is re-run after this
phase, the output should tell us one of:

**Case A — Improvement 1 points directly at the cause:**
```
interrupted at: ffff800000101xxx idle()+0x18
```
→ CPU=1 was in idle loop. The fault at `rip=0x87` means something corrupted
its return address or function pointer. Page poison will tell us if it's
freed kernel stack memory.

**Case B — Flight recorder shows the timeline:**
```
CPU=1 TLB_RECV vaddr=0 (full flush)
CPU=1 [crash at rip=0x87]
```
→ The CR3 reload in the TLB_RECV handler is the proximate cause.
We'd then check if the CR3 reload is flushing something it shouldn't
(e.g., kernel mapping not present in current CR3).

**Case C — Page poison shows use-after-free:**
```
rip=a5a5a5a5a5a5a587 (or similar 0xa5 pattern near 0x87)
```
→ A freed kernel page (stack? page table?) is being dereferenced as code.

Any of these outcomes breaks the analytic deadlock we've been in.

---

## Relationship to Phase 4

Phase 3.5 does not change any M6 functionality. All work is additive:
new diagnostic output, better error messages, a new `flight_recorder` module.
No existing syscalls, process management, or scheduler code is modified.

Phase 4 (TLB Shootdown + Thread Safety) resumes after this phase. The
specific failing test is `make test-threads-smp PROFILE=performance` which
crashes before `RUN(thread_create_join)` with `CPU=1 rip=0x87`. Phase 3.5
tooling is expected to identify the root cause in a single test run.
