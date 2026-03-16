# Platform / HAL

The `kevlar_platform` crate is Ring 0 in the ringkernel architecture. It is the only
crate that may contain `unsafe` code; everything above it uses `#![deny(unsafe_code)]`
or `#![forbid(unsafe_code)]`.

## What the Platform Does

| Subsystem | Responsibility |
|---|---|
| Paging | Physical frame allocation, page table construction, PCID, 4 KB/2 MB mappings, CoW refcounts |
| Context switch | Saving/restoring GP registers, xsave FPU/SSE/AVX state, FSBASE (TLS) |
| User-kernel copy | Alignment-aware `rep movsq` with `access_ok()` validation and fault probes |
| SMP | AP boot (INIT-SIPI-SIPI on x86, PSCI on ARM64), TLB shootdown IPI |
| IRQ | IDT/GIC setup, APIC/GIC EOI, IRQ routing |
| Boot | GDT, TSS, SYSCALL/SYSRET MSRs, EFER (LME\|NXE), multiboot2 |
| Timer | LAPIC timer at 100 Hz via TSC calibration |
| TSC clock | PIT-calibrated, fixed-point nanosecond conversion |
| vDSO | 4 KB ELF with `__vdso_clock_gettime` (~10 ns, no syscall) |
| Locks | Three SpinLock variants for different interrupt/preemption requirements |
| Randomness | RDRAND / RDSEED wrappers |
| Memory ops | Custom `memcpy`, `memset`, `memcmp` (no SSE; kernel runs with SSE disabled) |
| Flight recorder | Per-CPU lock-free ring buffers for crash diagnostics |
| Stack cache | Per-CPU warm kernel stack cache for fast fork |

## SMP Boot

### x86\_64: INIT-SIPI-SIPI

Application Processors are brought online via the Intel INIT-SIPI-SIPI protocol:

1. BSP allocates a kernel stack and per-CPU local storage for each AP.
2. BSP writes the CR3 (page table root) and stack pointer to the trampoline page
   at physical address 0x8000.
3. BSP sends INIT IPI → 10 ms delay → SIPI (vector 0x08 = page 0x8000) → 200 µs
   delay → second SIPI.
4. AP wakes in 16-bit real mode, transitions through protected mode to long mode,
   loads the BSP's CR3, and jumps to `ap_rust_entry`.
5. AP initializes its own GDT, IDT, TSS, LAPIC timer, and per-CPU TLS via GSBASE.
6. AP increments `AP_ONLINE_COUNT` and enters the kernel's idle loop.

```asm
; AP trampoline (platform/x64/ap_trampoline.S) — runs at physical 0x8000
.code16
    cli
    lgdt ap_tram_gdtr        ; Load embedded GDT
    mov cr0, PE              ; Enter protected mode
    jmp 0x0018:ap_tram_pm32  ; Far jump to 32-bit code
.code32
    mov cr3, [ap_tram_cr3]   ; Load page tables (written by BSP)
    set PAE+PGE in CR4
    set EFER.LME+NXE
    set CR0.PG               ; Enable paging → long mode
    jmp 0x0008:ap_tram_lm64
.code64
    mov rsp, [ap_tram_stack] ; Load kernel stack (written by BSP)
    jmp long_mode            ; Enter boot.S → ap_rust_entry
```

### ARM64: PSCI CPU\_ON

APs are started via PSCI `CPU_ON` hypercalls with the target MPIDR and entry address.
Each AP loads its stack and per-CPU storage from shared atomics, then enters the
kernel's idle loop.

### TLB Shootdown

When `mprotect` or `munmap` modifies page table entries, the local CPU performs
`invlpg` for each affected page, then sends a single IPI to all remote CPUs.
Remote CPUs reload CR3 (full flush) or `invlpg` the specific address. A bitmask
(`TLB_SHOOTDOWN_PENDING`) tracks which CPUs have acknowledged, with a busy-wait
on the sender.

The `lock_preempt()` lock variant keeps interrupts enabled during the wait so
remote CPUs can receive the IPI without deadlocking.

## Context Switch

Register save/restore is handled in assembly (`platform/x64/usermode.S`):

```asm
do_switch_thread:
    push rbp, rbx, r12-r15, rflags    ; Save callee-saved registers
    mov [rdi], rsp                     ; Store prev RSP
    mov byte ptr [rdx], 1             ; Store-release: context_saved = true
    mov rsp, [rsi]                     ; Load next RSP
    pop rflags, r15-r12, rbx, rbp     ; Restore callee-saved registers
    ret                                ; Jump to next thread's saved RIP
```

FPU/SSE/AVX state is saved and restored via `xsave64`/`xrstor64` around every
context switch. The xsave area is one page (4 KB) per task.

### Xsave Initialization

Fresh xsave areas must initialize `FCW = 0x037F` (x87 default mask) and
`MXCSR = 0x1F80` (SSE default). Without this, zeroed xsave causes a
`#XM` (SIMD Floating Point) exception on the first SSE instruction.

Fork copies the parent's xsave area to the child to preserve FPU state.

## SpinLock Variants

Three lock types for different contexts:

```rust
// Standard: disables interrupts (cli/sti), prevents IRQ-context deadlock
lock()          → SpinLockGuard (saves/restores RFLAGS)

// No-IRQ: skips cli/sti for locks never accessed from IRQ context
// Eliminates ~100 cycles of pushfq/cli/sti overhead
lock_no_irq()   → SpinLockGuardNoIrq

// Preempt-only: keeps interrupts ENABLED, disables preemption
// Used for locks held during TLB shootdown IPI (must allow IPI delivery)
lock_preempt()  → SpinLockGuardPreempt
```

`lock_no_irq` is used for the FD table, root\_fs, VMA lookups, and other structures
only accessed from syscall/thread context. `lock_preempt` is used for the page table
lock during TLB shootdown sequences.

## User-Mode Entry

```
enter_usermode(task)
    ├── New thread: userland_entry → sanitize registers → swapgs → iretq
    └── Fork child: forked_child_entry → restore syscall state → rax=0 → swapgs → iretq
```

Syscall entry uses `SYSCALL/SYSRET` (MSR-based fast path). The kernel receives the
syscall number in `rax` and arguments in `rdi, rsi, rdx, r10, r8, r9`.

## Usercopy

`copy_from_user` and `copy_to_user` (`platform/x64/usercopy.S`) use alignment-aware
bulk copy:

```asm
    ; Align destination to 8-byte boundary
    rep movsb           ; (up to 7 bytes)
    ; Bulk copy in 8-byte chunks
    rep movsq
    ; Copy trailing bytes
    rep movsb
```

Six probe points in the assembly are recognized by the page fault handler. If a fault
occurs at any probe point, the handler treats it as a user page fault (demand paging)
rather than a kernel crash. This allows usercopy to transparently fault in unmapped
user pages.

An optional trace ring buffer records all usercopy operations (destination, source,
length, return address) for debugging.

## Timer and TSC

The TSC is calibrated at boot using the PIT (Programmable Interval Timer):

```rust
// Measure TSC ticks in a 10 ms PIT window
let tsc_delta = tsc_end - tsc_start;
let freq = tsc_delta * PIT_HZ / pit_count;

// Fixed-point multiplier: avoids u64 division at runtime
let ns_mult = (1_000_000_000u128 << 32) / freq as u128;

// At runtime: ns = (delta * ns_mult) >> 32
```

The LAPIC timer is programmed in periodic mode at 100 Hz (10 ms per tick). Every
3 ticks (30 ms), the scheduler preempts the current process.

## vDSO

A hand-crafted 4 KB ELF shared object is assembled at boot and mapped read+exec into
every process at `0x1000_0000_0000`. It contains `__vdso_clock_gettime` that reads the
TSC and converts to nanoseconds entirely in user space — no syscall needed.

musl/glibc discover the vDSO via the `AT_SYSINFO_EHDR` auxiliary vector entry. The ELF
contains `DT_HASH`, `DT_SYMTAB`, and `DT_STRTAB` for symbol resolution.

## Flight Recorder

Per-CPU lock-free ring buffers (64 entries each) record kernel events for post-mortem
crash analysis:

- `CTX_SWITCH` — context switch from/to PIDs
- `TLB_SEND` / `TLB_RECV` — TLB shootdown IPI send/acknowledge
- `MMAP_FAULT` — page fault address and handler
- `PREEMPT` — timer preemption
- `SYSCALL_IN` / `SYSCALL_OUT` — syscall entry/exit with number
- `SIGNAL` — signal delivery
- `IDLE` — CPU entered idle loop

On panic, the flight recorder dumps all CPU rings to the serial console.

## Architecture Variants

The platform crate has separate modules for x86\_64 (`platform/x64/`) and ARM64
(`platform/arm64/`). Both expose the same safe API to the kernel.

| Feature | x86\_64 | ARM64 |
|---|---|---|
| Syscall entry | SYSCALL/SYSRET MSRs | SVC instruction |
| Timer | APIC + TSC calibration | ARM generic timer (CNTFRQ\_EL0) |
| Interrupt controller | APIC (QEMU q35) | GIC-v2 (QEMU virt) |
| SMP boot | INIT-SIPI-SIPI | PSCI CPU\_ON |
| vDSO | Yes | Not yet |
| QEMU target | `q35 -cpu Icelake-Server` | `virt -cpu cortex-a72` |

## Safety Model

The platform crate enforces safety through:

1. **No public raw pointer APIs.** All pointer-taking functions return `Result` and
   validate bounds before any dereference.
2. **`Pod` constraint on user copies.** Prevents references from crossing the boundary.
   `Pod` requires `Copy + repr(C)` — no types with drop glue.
3. **SAFETY comments.** Every `unsafe` block has a `// SAFETY:` comment explaining
   the invariant.
4. **`access_ok()` on all user addresses.** Skipped only in the Ludicrous profile.
5. **Fault probes in usercopy.** Kernel page faults at known probe points are treated
   as user page faults, not panics.

The kernel crate (`#![deny(unsafe_code)]`) has 7 annotated `#[allow(unsafe_code)]`
sites across 4 files, each with a documented justification.
