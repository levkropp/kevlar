# M6 Phase 1: SMP Boot

Kevlar now boots all Application Processors.  On a 4-vCPU QEMU guest,
the kernel prints "CPU (LAPIC 1) online … smp: 3 AP(s) online, total 4
CPU(s)" before handing control to the shell.  This post walks through
the INIT-SIPI-SIPI protocol, the 16→64-bit AP trampoline, ACPI MADT
discovery, and the two bugs that kept the APs silent until the very end.

---

## Why SMP matters here

Kevlar's long-term goal is running Wine — a workload that spawns dozens
of threads and expects them to make real parallel progress.  A
single-CPU kernel can schedule threads, but every blocking call stalls
everything else.  SMP is the prerequisite for M6 Phase 2 (per-CPU run
queues) and, ultimately, for any realistic multi-threaded workload.

It also forces every shared data structure to be safe under concurrent
access.  We already had `SpinLock` — but it contained a debug assertion
that a lock contended while interrupts are disabled is always a deadlock
("we're single-CPU, so if the lock is held it must be by us").  That
assertion is gone now; real lock contention is expected.

---

## Waking the APs: INIT-SIPI-SIPI

After power-on, every processor except the Bootstrap Processor (BSP)
parks itself in a halted state, waiting for an Inter-Processor Interrupt
from the BSP to tell it where to begin executing.  Intel's SDM
prescribes the *INIT-SIPI-SIPI* sequence:

1. **INIT IPI** — resets the AP's internal state.
2. **10 ms delay**
3. **STARTUP IPI (SIPI)** — carries a *vector* byte (0x08 → start at
   physical 0x8000).  The AP wakes in 16-bit real mode at CS:IP =
   `(vector<<8):0x0000`.
4. **200 µs delay**
5. **Second SIPI** — in case the first was missed.

IPIs are written to the Local APIC's *Interrupt Command Register* (ICR)
via MMIO at `0xfee00300` (low half) and `0xfee00310` (high half, which
selects the destination APIC ID):

```rust
// ICR command values
const ICR_INIT: u32 = 0x00004500; // Delivery=INIT, Level=Assert
const ICR_SIPI: u32 = 0x00000600; // Delivery=StartUp (vector in [7:0])

pub unsafe fn send_sipi(apic_id: u8, vector: u8) {
    lapic_write(ICR_HIGH_OFF, (apic_id as u32) << 24);
    lapic_write(ICR_LOW_OFF, ICR_SIPI | vector as u32);
    wait_icr_idle();
}
```

APIC IDs come from the ACPI MADT — more on that below.

---

## The AP trampoline

An AP wakes in 16-bit real mode at physical 0x8000.  To reach the
64-bit kernel it must re-run the same mode transitions as the BSP:

```
16-bit real mode  →  32-bit protected mode  →  64-bit long mode
```

The trampoline lives in `platform/x64/ap_trampoline.S` and is placed
in its own `.trampoline` ELF section with **VMA = 0x8000** (so the
assembler generates the correct absolute addresses for real-mode
references) but loaded at a physical address inside the main kernel
image.  Before the BSP sends any SIPIs it calls `copy_trampoline()` to
memcpy the 182-byte blob to physical 0x8000:

```rust
unsafe fn copy_trampoline() {
    extern "C" {
        static __trampoline_start: u8;
        static __trampoline_end:   u8;
        static __ap_trampoline_image: u8; // LOADADDR(.trampoline) — physical LMA
    }
    let size = (&raw const __trampoline_end   as usize)
             - (&raw const __trampoline_start as usize);
    let src = ((&raw const __ap_trampoline_image as usize)
               | 0xffff_8000_0000_0000) as *const u8;  // paddr → vaddr
    let dst = 0x8000usize as *mut u8;
    core::ptr::copy_nonoverlapping(src, dst, size);
}
```

The trampoline carries two data words that the BSP writes before each
SIPI:

```asm
.global ap_tram_cr3
ap_tram_cr3:   .long 0   // physical PML4 address (BSP's page table)

.global ap_tram_stack
ap_tram_stack: .quad 0   // virtual kernel stack top for this AP
```

After enabling paging it jumps to `long_mode` in `boot.S` — the same
label used by the BSP.  `boot.S` reads the LAPIC ID register; non-zero
means AP, which dispatches to `ap_rust_entry`:

```rust
#[unsafe(no_mangle)]
unsafe extern "C" fn ap_rust_entry(lapic_id: u32) -> ! {
    let cpu_local_vaddr = VAddr::new(smp::AP_CPU_LOCAL.load(Ordering::Acquire));
    ap_common_setup(cpu_local_vaddr);   // CR4/FSGSBASE/XSAVE, GDT, IDT, syscall

    info!("CPU (LAPIC {}) online", lapic_id);
    smp::AP_ONLINE_COUNT.fetch_add(1, Ordering::Release);

    loop { super::idle::idle(); }
}
```

APs are started one at a time; the BSP waits up to 200 ms for each AP
to increment `AP_ONLINE_COUNT` before proceeding to the next.

---

## ACPI MADT discovery

To know *which* APIC IDs to wake, we need the ACPI
*Multiple APIC Description Table* (MADT).  The minimal parser in
`platform/x64/acpi.rs` does exactly what's necessary and nothing more:

1. Scan 0xE0000–0xFFFFF (the BIOS extended area) for the `"RSD PTR "` signature.
2. Follow `RSDP.rsdt_address` to the RSDT.
3. Walk RSDT entries (32-bit physical pointers) for the table with signature `"APIC"`.
4. Iterate MADT interrupt-controller structures; collect Type-0 (Processor Local APIC)
   entries that have the *Processor Enabled* flag set.

No heap, no ACPI library — just raw pointer arithmetic over physical
memory.  With QEMU `-smp 4` the parser finds four LAPIC entries (IDs
0–3); the BSP skips its own ID and wakes the other three.

---

## Two bugs, one at a time

### Bug 1: `.mb_stub` broke the kernel entry point

The M6 branch had added a `.mb_stub` ELF section at physical address
0x4000 to ensure the multiboot1 magic landed within QEMU's 8 KB scanner
window.  That turned out to be unnecessary — the existing multiboot1
header in `.boot` sits at file offset ~0x1028, well inside 8 KB.

The more important effect: QEMU's multiboot loader sets
`FW_CFG_KERNEL_ADDR = elf_low`, where `elf_low` is the minimum paddr
across all PT_LOAD segments with `p_filesz > 0`.  Adding the stub at
paddr 0x4000 moved `elf_low` from **0x100000 to 0x4000**, which shifted
the entry-point calculation in the multiboot DMA ROM and made it jump to
**0x100001** (one byte into the multiboot2 magic) instead of 0x100034.
Triple fault, silent death.

Fix: remove `.mb_stub` entirely.

### Bug 2: the page allocator ate the trampoline

The trampoline ELF segment uses
`AT(__kernel_image_end)` so its physical load address equals the first
byte of free RAM.  The bootinfo parser reports this same address as the
start of the available heap.  `page_allocator::init()` claimed that
range, and the very first page allocation zeroed physical 0xc4b000 —
exactly where the trampoline bytes had been placed.

The fix is a one-line reorder: call `copy_trampoline()` *before*
`page_allocator::init()`:

```rust
unsafe extern "C" fn bsp_early_init(boot_magic: u32, boot_params: u64) -> ! {
    serial::early_init();
    vga::init();
    logger::init();

    // Must run before page_allocator::init() claims physical 0xc4b000.
    copy_trampoline();

    let boot_info = bootinfo::parse(boot_magic, PAddr::new(boot_params as usize));
    page_allocator::init(&boot_info.ram_areas);
    // …
}
```

The GDB session that caught this was clean: break at line 160
(before `page_allocator::init()`), read `0xffff800000c4b000` —
`0xfa 0xfc 0x31 0xc0` (CLI, CLD, XOR AX,AX — correct).  After `init()`,
same address shows `0x00`.  Case closed.

---

## Results

```
acpi: RSDP at 0xf64f0
acpi: found 4 Local APIC(s)
CPU (LAPIC 1) online
CPU (LAPIC 2) online
CPU (LAPIC 3) online
smp: 3 AP(s) online, total 4 CPU(s)
Booting Kevlar...
```

Verified under both QEMU TCG and KVM with `-smp 4`.  All 25 existing
tests pass; the 6 ext2 failures are a separate in-progress item.

---

## What's next

The APs are online but idle — they sit in `hlt` loops waiting for work.
M6 Phase 2 will give each CPU its own run queue and implement work
stealing so that runnable tasks spread across all available cores.  That
requires rethinking the global scheduler lock, adding per-CPU
`cpu_local` scheduler state, and a dequeue path triggered from the LAPIC
timer interrupt that already fires on every CPU.

| Phase | Description | Status |
|-------|-------------|--------|
| M6 Phase 1 | SMP boot (INIT-SIPI-SIPI, trampoline, MADT) | ✅ Done |
| M6 Phase 2 | Per-CPU run queues + LAPIC timer preemption | ✅ Done |
| M6 Phase 3 | Futex wake-on-CPU, pthread_create end-to-end | 🔄 Next |
