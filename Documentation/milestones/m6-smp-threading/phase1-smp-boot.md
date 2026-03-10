# Phase 1: SMP Boot

**Goal:** Wake secondary CPUs (Application Processors), set up per-CPU state,
and have all cores running idle loops in the kernel. No scheduler changes yet —
APs just spin in their idle thread.

## x86_64: INIT-SIPI-SIPI Protocol

### AP Startup Sequence

The BSP (Bootstrap Processor) wakes each AP using the INIT-SIPI-SIPI sequence
via the Local APIC:

1. **Write AP trampoline code** to a known physical address below 1 MiB
   (e.g., 0x8000). The trampoline starts in 16-bit real mode.
2. **Send INIT IPI** to target AP (or broadcast to all APs).
3. **Wait 10ms.**
4. **Send STARTUP IPI** (SIPI) with vector = trampoline page (0x08 for 0x8000).
5. **Wait 200us, send second SIPI** (some CPUs need two).
6. AP executes trampoline: real mode → protected mode → long mode → jump to
   Rust `ap_entry()`.

### AP Trampoline (assembly)

```asm
; AP starts here in 16-bit real mode at physical address 0x8000
[BITS 16]
ap_trampoline:
    cli
    ; Load a GDT with 32-bit and 64-bit segments
    lgdt [ap_gdt_ptr]
    ; Enable protected mode
    mov eax, cr0
    or  eax, 1
    mov cr0, eax
    ; Far jump to 32-bit code
    jmp 0x08:ap_protected

[BITS 32]
ap_protected:
    ; Set up 64-bit page tables (use BSP's page table)
    mov eax, [bsp_cr3]
    mov cr3, eax
    ; Enable PAE + PGE
    mov eax, cr4
    or  eax, 0x20 | 0x80   ; PAE | PGE
    mov cr4, eax
    ; Enable long mode (EFER.LME + EFER.NXE)
    mov ecx, 0xC0000080
    rdmsr
    or  eax, 0x0900         ; LME | NXE
    wrmsr
    ; Enable paging
    mov eax, cr0
    or  eax, 0x80000000
    mov cr0, eax
    ; Far jump to 64-bit code
    jmp 0x18:ap_long_mode

[BITS 64]
ap_long_mode:
    ; Load per-CPU kernel stack
    mov rsp, [ap_stack_ptr]
    ; Load per-CPU GS base for per-CPU data
    ; Jump to Rust ap_entry(cpu_id)
    call ap_entry
```

### BSP Discovery

Detect available CPUs via:
1. **ACPI MADT (Multiple APIC Description Table):** Parse for Processor Local
   APIC entries. Each entry has an APIC ID and processor-enabled flag.
2. **Fallback:** MP Configuration Table (older systems).

For QEMU, ACPI MADT is always available. Parse it during early boot to count
CPUs and collect APIC IDs.

## ARM64: PSCI Boot Protocol

ARM64 uses PSCI (Power State Coordination Interface) to bring up secondary
cores:

```rust
fn start_ap(cpu_id: u64, entry_addr: u64) {
    // PSCI CPU_ON: SMC #0 with function ID 0xC4000003
    let result = psci_cpu_on(cpu_id, entry_addr, 0);
}
```

Each AP starts at `entry_addr` in EL1 with MMU off. The AP trampoline:
1. Set up page tables (use BSP's TTBR1_EL1 for kernel mappings)
2. Enable MMU (SCTLR_EL1)
3. Set up exception vectors (VBAR_EL1)
4. Load per-CPU stack pointer
5. Jump to Rust `ap_entry(cpu_id)`

QEMU virt machine provides PSCI via the built-in firmware.

## Per-CPU Data

Each CPU needs its own:

| Data | x86_64 storage | ARM64 storage |
|------|---------------|--------------|
| CPU ID | GS-based percpu struct | TPIDR_EL1 percpu struct |
| Current process | GS-based percpu struct | TPIDR_EL1 percpu struct |
| Kernel stack | Separate page per CPU | Separate page per CPU |
| Idle thread | Per-CPU Process struct | Per-CPU Process struct |
| GDT + TSS | Per-CPU (x86 only) | N/A |
| Interrupt stack | IST in TSS (x86 only) | SP_EL1 per CPU |

### PerCpu struct

```rust
#[repr(C)]
pub struct PerCpu {
    /// Self-pointer for GS-relative addressing (x86_64).
    self_ptr: *const PerCpu,
    /// CPU index (0 = BSP).
    pub cpu_id: u32,
    /// Currently running process on this CPU.
    pub current: *const Process,
    /// This CPU's idle thread.
    pub idle_thread: Arc<Process>,
    /// Kernel stack top for this CPU.
    pub kernel_stack_top: u64,
    /// Preemption disable count (0 = preemptible).
    pub preempt_count: u32,
}
```

x86_64: set GS base to point to this CPU's `PerCpu` via `wrmsrl(MSR_GS_BASE)`.
ARM64: set TPIDR_EL1 to point to this CPU's `PerCpu`.

Access pattern: `current_cpu().current` replaces the current global
`current_process()`. The global function becomes a thin wrapper that reads
from per-CPU data.

## Initialization Order

1. BSP parses ACPI MADT → count CPUs, collect APIC IDs
2. BSP allocates per-CPU data structures and stacks
3. BSP sets up its own PerCpu (cpu_id=0)
4. BSP writes AP trampoline to low memory
5. For each AP: send INIT-SIPI-SIPI, wait for AP to signal ready
6. AP executes trampoline, enters `ap_entry(cpu_id)`
7. `ap_entry`: set up per-CPU data, enable local APIC, enable interrupts,
   enter idle loop
8. BSP continues normal boot (init process, etc.)

## What APs Do Initially

After Phase 1, APs simply run their idle thread (halt loop). They respond to
interrupts (timer, IPI) but don't run user processes yet — that comes in
Phase 2 with the SMP scheduler.

```rust
fn ap_entry(cpu_id: u32) -> ! {
    setup_percpu(cpu_id);
    setup_local_apic();
    enable_interrupts();
    info!("CPU {} online", cpu_id);
    // Signal BSP that we're ready.
    AP_READY_COUNT.fetch_add(1, Ordering::Release);
    // Enter idle loop.
    loop { halt(); }
}
```

## Reference Sources

- Intel SDM Volume 3, Chapter 8 — MP Initialization Protocol
- ARM Architecture Reference Manual — PSCI specification
- FreeBSD `sys/x86/x86/mp_x86.c` (BSD-2-Clause) — AP boot
- OSDev wiki — SMP, APIC, MADT parsing

## Testing

- `QEMU -smp 4`: all 4 CPUs reach idle loop, serial output confirms
- `/proc/cpuinfo` shows 4 processors (Phase 4 of M5 adds this file)
- No panics or hangs during AP startup
- BSP continues normal boot (shell prompt appears)
- ARM64: same test with `QEMU -smp 4` on virt machine
