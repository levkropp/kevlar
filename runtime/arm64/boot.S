// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// ARM64 boot code for Kevlar on QEMU virt machine.
// Entry: EL1, MMU off, x0 = DTB physical address.

.section ".boot", "ax"
.global _start
_start:
    // Save DTB address (passed by QEMU in x0).
    mov     x19, x0

    // Disable interrupts.
    msr     daifset, #0xf

    // Enable FP/NEON (CPACR_EL1.FPEN = 0b11).
    mov     x0, #(3 << 20)
    msr     cpacr_el1, x0
    isb

    // Set up the boot stack (physical address).
    adrp    x0, __boot_stack
    add     sp, x0, :lo12:__boot_stack

    // ---------------------------------------------------------------
    // Build initial page tables.
    // We use 2MB block mappings (level 2 / PMD) to map 0-4GB.
    // TTBR1 maps kernel VA (0xffff_0000_xxxx_xxxx → physical).
    // TTBR0 maps identity (0x0000_xxxx_xxxx → physical, for boot).
    // ---------------------------------------------------------------

    // Zero out all page table pages.
    adrp    x0, __kernel_pgd_ttbr1
    add     x0, x0, :lo12:__kernel_pgd_ttbr1
    mov     x1, #0
    mov     x2, #(12 * 4096)       // 12 pages: 2*(PGD+PUD+4*PMD)
    bl      boot_memzero

    // --- TTBR1 (kernel) page tables ---

    // PGD[0] → PUD
    adrp    x0, __kernel_pgd_ttbr1
    add     x0, x0, :lo12:__kernel_pgd_ttbr1
    adrp    x1, __kernel_pud_ttbr1
    add     x1, x1, :lo12:__kernel_pud_ttbr1
    orr     x1, x1, #0x3           // Valid + Table
    str     x1, [x0]

    // PUD[0..3] → PMD[0..3]
    adrp    x0, __kernel_pud_ttbr1
    add     x0, x0, :lo12:__kernel_pud_ttbr1
    adrp    x1, __kernel_pmd_ttbr1
    add     x1, x1, :lo12:__kernel_pmd_ttbr1
    mov     x2, #4
1:  orr     x3, x1, #0x3           // Valid + Table
    str     x3, [x0], #8
    add     x1, x1, #4096
    subs    x2, x2, #1
    b.ne    1b

    // PMD: fill 4*512 entries with 2MB blocks mapping 0 - 4GB.
    adrp    x0, __kernel_pmd_ttbr1
    add     x0, x0, :lo12:__kernel_pmd_ttbr1
    mov     x1, #0                  // Physical address starts at 0
    mov     x2, #(4 * 512)          // Total entries
    bl      fill_pmd_entries

    // --- TTBR0 (identity) page tables --- same structure ---

    // PGD[0] → PUD
    adrp    x0, __kernel_pgd_ttbr0
    add     x0, x0, :lo12:__kernel_pgd_ttbr0
    adrp    x1, __kernel_pud_ttbr0
    add     x1, x1, :lo12:__kernel_pud_ttbr0
    orr     x1, x1, #0x3
    str     x1, [x0]

    // PUD[0..3] → PMD[0..3]
    adrp    x0, __kernel_pud_ttbr0
    add     x0, x0, :lo12:__kernel_pud_ttbr0
    adrp    x1, __kernel_pmd_ttbr0
    add     x1, x1, :lo12:__kernel_pmd_ttbr0
    mov     x2, #4
1:  orr     x3, x1, #0x3
    str     x3, [x0], #8
    add     x1, x1, #4096
    subs    x2, x2, #1
    b.ne    1b

    // PMD: fill 4*512 entries with 2MB blocks mapping 0 - 4GB.
    adrp    x0, __kernel_pmd_ttbr0
    add     x0, x0, :lo12:__kernel_pmd_ttbr0
    mov     x1, #0
    mov     x2, #(4 * 512)
    bl      fill_pmd_entries

    // ---------------------------------------------------------------
    // Configure and enable the MMU.
    // ---------------------------------------------------------------

    // MAIR_EL1: attr0 = Device-nGnRnE (0x00), attr1 = Normal WB (0xFF)
    mov     x0, #0xFF00
    msr     mair_el1, x0

    // TCR_EL1:
    //   T0SZ = 16 (48-bit user VA)
    //   T1SZ = 16 (48-bit kernel VA)
    //   TG0  = 0b00 (4KB granule)
    //   TG1  = 0b10 (4KB granule)
    //   IPS  = 0b101 (48-bit PA)
    //   IRGN0/ORGN0 = 0b01 (WB-WA)
    //   IRGN1/ORGN1 = 0b01 (WB-WA)
    //   SH0/SH1 = 0b11 (Inner Shareable)
    ldr     x0, =0x00000025B5103510
    msr     tcr_el1, x0

    // Set TTBR0 and TTBR1.
    adrp    x0, __kernel_pgd_ttbr0
    add     x0, x0, :lo12:__kernel_pgd_ttbr0
    msr     ttbr0_el1, x0

    adrp    x0, __kernel_pgd_ttbr1
    add     x0, x0, :lo12:__kernel_pgd_ttbr1
    msr     ttbr1_el1, x0

    // Invalidate TLBs.
    tlbi    vmalle1
    dsb     sy
    isb

    // Enable MMU via SCTLR_EL1.
    mrs     x0, sctlr_el1
    orr     x0, x0, #(1 << 0)      // M    = MMU enable
    orr     x0, x0, #(1 << 2)      // C    = data cache enable
    orr     x0, x0, #(1 << 12)     // I    = instruction cache enable
    orr     x0, x0, #(1 << 16)     // nTWI = don't trap WFI at EL1
    orr     x0, x0, #(1 << 18)     // nTWE = don't trap WFE at EL1
    orr     x0, x0, #(1 << 26)     // UCI  = EL0 cache maintenance
    bic     x0, x0, #(1 << 25)     // EE   = little-endian
    bic     x0, x0, #(1 << 19)     // WXN  = no write-execute restriction
    msr     sctlr_el1, x0
    isb

    // Jump to the kernel virtual address.
    ldr     x0, =long_mode
    br      x0

// ---------------------------------------------------------------
// Helper: fill PMD entries with 2MB block descriptors.
//   x0 = PMD table base
//   x1 = starting physical address
//   x2 = number of entries
// ---------------------------------------------------------------
fill_pmd_entries:
    // Block descriptor: Valid(0) | Block(0) | AF(10) | SH(9:8) | AttrIndx(4:2)
    // For phys < 0x40000000: Device memory (AttrIndx=0)
    // For phys >= 0x40000000: Normal memory (AttrIndx=1)
    mov     x4, #0x40000000         // RAM boundary
1:
    cmp     x1, x4
    b.hs    2f
    // Device memory: AttrIndx=0, SH=OSH(0b10), AF=1
    mov     x3, #((0 << 2) | (0b10 << 8) | (1 << 10))
    orr     x3, x3, #0x1           // Valid + Block
    orr     x3, x3, x1
    b       3f
2:
    // Normal memory: AttrIndx=1, SH=ISH(0b11), AF=1
    mov     x3, #((1 << 2) | (0b11 << 8) | (1 << 10))
    orr     x3, x3, #0x1           // Valid + Block
    orr     x3, x3, x1
3:
    str     x3, [x0], #8
    add     x1, x1, #0x200000      // Next 2MB block
    subs    x2, x2, #1
    b.ne    1b
    ret

// ---------------------------------------------------------------
// Helper: zero memory.
//   x0 = start, x1 = value (0), x2 = byte count
// ---------------------------------------------------------------
boot_memzero:
    cbz     x2, 2f
1:  strb    w1, [x0], #1
    subs    x2, x2, #1
    b.ne    1b
2:  ret

// ---------------------------------------------------------------
// Long mode: running with MMU enabled at kernel virtual address.
// ---------------------------------------------------------------
.text
long_mode:
    // Adjust stack pointer to virtual address.
    ldr     x0, =0xffff000000000000
    mov     x1, sp
    add     sp, x0, x1

    // Clear .bss section.
    ldr     x0, =__bss
    ldr     x1, =__bss_end
    mov     w2, #0
1:  cmp     x0, x1
    b.hs    2f
    strb    w2, [x0], #1
    b       1b
2:

    // Set VBAR_EL1 (exception vector table).
    ldr     x0, =exception_vectors
    msr     vbar_el1, x0
    isb

    // Ensure WFI/WFE are not trapped at EL1.
    mrs     x0, sctlr_el1
    orr     x0, x0, #(1 << 16)     // nTWI
    orr     x0, x0, #(1 << 18)     // nTWE
    msr     sctlr_el1, x0
    isb

    // Clear frame pointer for backtrace.
    mov     x29, #0

    // Call Rust entry: bsp_early_init(dtb_paddr: u64)
    mov     x0, x19
    bl      bsp_early_init

    // Should not return.
halt:
    wfi
    b       halt
