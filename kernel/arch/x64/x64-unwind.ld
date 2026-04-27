OUTPUT_FORMAT("elf64-x86-64");
ENTRY(boot);

VMA_OFFSET  = 0xffff800000000000;
KERNEL_BASE = VMA_OFFSET + 0x100000;

PHDRS {
    phdr_boot         PT_LOAD        FLAGS(5);   /* R+X - boot code at 0x100000      */
    phdr_text         PT_LOAD        FLAGS(5);   /* R+X - kernel text                 */
    phdr_rodata       PT_LOAD        FLAGS(4);   /* R   - read-only data              */
    phdr_eh_frame     PT_LOAD        FLAGS(4);   /* R   - unwind tables               */
    phdr_eh_frame_hdr PT_GNU_EH_FRAME FLAGS(4);  /* R   - eh_frame_hdr (unwinder)     */
    phdr_data         PT_LOAD        FLAGS(6);   /* R+W - read-write data             */
    phdr_bss          PT_LOAD        FLAGS(6);   /* R+W - BSS                         */
    phdr_trampoline   PT_LOAD        FLAGS(5);   /* R+X - AP trampoline image         */
}

SECTIONS {
    . = KERNEL_BASE - VMA_OFFSET;

    .boot : {
        __kernel_image = .; /* physical address */
        KEEP(*(.boot));
    } :phdr_boot

    . = KERNEL_BASE + SIZEOF(.boot);

    .text : AT(ADDR(.text) - VMA_OFFSET) {
        *(.text);
        *(.text.*);
    } :phdr_text

    __etext = .;

    .rodata : AT(ADDR(.rodata) - VMA_OFFSET) {
        *(.rodata);
        *(.rodata.*);

        /* kABI exported-symbol table.  See arm64.ld for details. */
        . = ALIGN(8);
        __ksymtab_start = .;
        KEEP(*(.ksymtab));
        __ksymtab_end = .;

        . = ALIGN(4096);
        __cpu_local = .;
        KEEP(*(.cpu_local_head));
        KEEP(*(.cpu_local));
        __cpu_local_end = .;
        __cpu_local_size = __cpu_local_end - __cpu_local;
    } :phdr_rodata

    /* Unwind tables — required by the unwinding crate for catch_unwind. */
    .eh_frame : AT(ADDR(.eh_frame) - VMA_OFFSET) {
        __eh_frame = .;
        KEEP(*(.eh_frame));
        KEEP(*(.eh_frame.*));
        __eh_frame_end = .;
    } :phdr_eh_frame

    .eh_frame_hdr : AT(ADDR(.eh_frame_hdr) - VMA_OFFSET) {
        KEEP(*(.eh_frame_hdr));
        KEEP(*(.eh_frame_hdr.*));
    } :phdr_eh_frame_hdr

    .data : AT(ADDR(.data) - VMA_OFFSET) {
        *(.data);
        *(.data.*);

        *(.got*);

        __bsp_cpu_local = .;
        . += __cpu_local_size;
        __bsp_cpu_local_end = .;
    } :phdr_data

    .bss : AT(ADDR(.bss) - VMA_OFFSET) {
        __bss = .;
        *(.bss);
        *(.bss.*);
        __bss_end = .;

        /* The kernel page table (physical addresses). */
        . = ALIGN(4096);
        __kernel_pml4 = . - VMA_OFFSET;
        . += 0x1000;
        __kernel_pdpt = . - VMA_OFFSET;
        . += 0x1000;
        __kernel_pd = . - VMA_OFFSET;
        . += 4 * 512 * 8; /* (# of PDPT entries) * (# of entries in PD) *
                             (size of entry) */

        /* The initial stack for BSP. We need reserve a large space since Rust
           tend to consume too much memory especially in the debug buid :/  */
        . += 0x10000;
        __boot_stack = . - VMA_OFFSET; /* physical address */

        . = ALIGN(4096);
        __kernel_image_end = . - VMA_OFFSET; /* physical address */
    } :phdr_bss

    /* AP trampoline: VMA=0x8000 so assembly uses correct absolute addresses;
       AT(__kernel_image_end) places bytes after BSS — keeps elf_low=0x100000.
       BSP copies to physical 0x8000 before sending SIPIs. */
    .trampoline 0x8000 : AT(__kernel_image_end) {
        __trampoline_start = .;
        KEEP(*(.trampoline));
        __trampoline_end = .;
    } :phdr_trampoline

    /* Physical (load) address of the trampoline image in the ELF. */
    __ap_trampoline_image = LOADADDR(.trampoline);
}
