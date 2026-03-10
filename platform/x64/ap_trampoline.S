// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// AP trampoline: executes at physical 0x8000 when an Application Processor
// is woken by the BSP via INIT-SIPI-SIPI.  Transitions the AP through
// 16-bit real mode → 32-bit protected mode → 64-bit long mode, then
// jumps to `long_mode` in boot.S (which reads the LAPIC ID and dispatches
// to ap_rust_entry).
//
// The trampoline is placed in the `.trampoline` section at VMA 0x8000 =
// physical 0x8000 (identity-mapped).  The multiboot loader copies it
// there at kernel load time.
//
// Before sending SIPI, the BSP writes two data fields:
//   ap_tram_cr3   — physical address of the BSP's PML4 (kernel page table)
//   ap_tram_stack — virtual address of this AP's kernel stack top
//
// GDT layout (mirrors the boot GDT so CS=8 is compatible with the real GDT):
//   0x00  null
//   0x08  64-bit code  (L=1, D/B=0) — used for long-mode entry and after
//   0x10  32-bit data  (G=1, D=1)   — used in protected-mode setup
//   0x18  32-bit code  (G=1, D=1)   — used during 16→32 transition

.section ".trampoline", "ax"
.global ap_trampoline_start
ap_trampoline_start:

// ── 16-bit real mode ──────────────────────────────────────────────────
.code16
    cli
    cld
    xorw    %ax, %ax
    movw    %ax, %ds
    movw    %ax, %es
    movw    %ax, %ss

    // Load the GDT embedded in this page.
    // data32 gives a 6-byte lgdt (2-byte limit + 4-byte base).
    data32 lgdt    ap_tram_gdtr

    // Enable protected mode: set CR0.PE.
    movl    %cr0, %eax
    orb     $1, %al
    movl    %eax, %cr0

    // Far jump to 32-bit code.  Encoded manually because GAS 16-bit far
    // jumps with 32-bit EIP are awkward: 0x66 0xEA <32-bit EIP> <16-bit CS>.
    .byte   0x66, 0xea          // data32 far jmp
    .long   ap_tram_pm32        // 32-bit EIP  (VMA = physical for low section)
    .word   0x0018              // CS = GDT[3] = 32-bit code segment

// ── 32-bit protected mode ─────────────────────────────────────────────
.align 4
.code32
ap_tram_pm32:
    movw    $0x10, %ax          // GDT[2] = 32-bit data
    movw    %ax, %ds
    movw    %ax, %es
    movw    %ax, %ss
    xorw    %ax, %ax
    movw    %ax, %fs
    movw    %ax, %gs

    // Load BSP's page table (physical PML4 address written by BSP).
    movl    ap_tram_cr3, %eax
    movl    %eax, %cr3

    // Enable PAE (bit 5) and PGE (bit 7).
    movl    %cr4, %eax
    orl     $0xa0, %eax
    movl    %eax, %cr4

    // Enable EFER.LME (bit 8) and EFER.NXE (bit 11).
    movl    $0xc0000080, %ecx
    rdmsr
    orl     $0x0900, %eax
    wrmsr

    // Enable paging (CR0.PG).
    movl    %cr0, %eax
    orl     $0x80000000, %eax
    movl    %eax, %cr0

    // Far jump to 64-bit code.  0xEA <32-bit EIP> <16-bit CS>.
    .byte   0xea                // far jmp (32-bit default operand in .code32)
    .long   ap_tram_lm64        // 32-bit EIP
    .word   0x0008              // CS = GDT[1] = 64-bit code segment

// ── 64-bit long mode (still at low physical/virtual address) ──────────
.align 4
.code64
ap_tram_lm64:
    xorw    %ax, %ax
    movw    %ax, %ds
    movw    %ax, %es
    movw    %ax, %ss

    // Load the per-AP kernel stack pointer (virtual addr, set by BSP).
    movq    ap_tram_stack(%rip), %rsp

    // Jump to `long_mode` in boot.S (high-half kernel virtual address).
    movabsq $long_mode, %rax
    jmpq    *%rax

// ── Data written by the BSP before sending SIPI ───────────────────────
.align 8
.global ap_tram_cr3
ap_tram_cr3:
    .long   0           // physical CR3 (BSP's __kernel_pml4)
    .long   0           // padding

.global ap_tram_stack
ap_tram_stack:
    .quad   0           // virtual AP kernel stack top

// ── Embedded GDT ──────────────────────────────────────────────────────
.align 8
ap_tram_gdt:
    .quad 0x0000000000000000    // GDT[0]: null
    .quad 0x00af9a000000ffff    // GDT[1]: 64-bit code  (selector 0x08)
    .quad 0x00cf92000000ffff    // GDT[2]: 32-bit data  (selector 0x10)
    .quad 0x00cf9a000000ffff    // GDT[3]: 32-bit code  (selector 0x18)
ap_tram_gdt_end:

// ── GDTR ──────────────────────────────────────────────────────────────
.align 4
ap_tram_gdtr:
    .word   ap_tram_gdt_end - ap_tram_gdt - 1
    .long   ap_tram_gdt         // physical = VMA for low-address section

.global ap_trampoline_end
ap_trampoline_end:
