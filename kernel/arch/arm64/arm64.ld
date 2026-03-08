OUTPUT_FORMAT("elf64-littleaarch64");
ENTRY(_start);

VMA_OFFSET  = 0xffff000000000000;
/* Kernel loaded at physical 0x40080000 by QEMU (after 512KB for DTB etc.) */
KERNEL_BASE = VMA_OFFSET + 0x40080000;

SECTIONS {
    /* Boot code at physical address, before MMU is enabled. */
    . = 0x40080000;

    .boot : {
        __kernel_image = .; /* physical address */
        KEEP(*(.boot));
    }

    . = KERNEL_BASE + SIZEOF(.boot);

    .text : AT(ADDR(.text) - VMA_OFFSET) {
        *(.text);
        *(.text.*);
    }

    .rodata : AT(ADDR(.rodata) - VMA_OFFSET) {
        *(.rodata);
        *(.rodata.*);

        . = ALIGN(4096);
        __cpu_local = .;
        KEEP(*(.cpu_local_head));
        KEEP(*(.cpu_local));
        __cpu_local_end = .;
        __cpu_local_size = __cpu_local_end - __cpu_local;
    }

    .data : AT(ADDR(.data) - VMA_OFFSET) {
        *(.data);
        *(.data.*);

        *(.got*);

        __bsp_cpu_local = .;
        . += __cpu_local_size;
        __bsp_cpu_local_end = .;
    }

    .bss : AT(ADDR(.bss) - VMA_OFFSET) {
        __bss = .;
        *(.bss);
        *(.bss.*);
        __bss_end = .;

        /*
         * Boot stack comes BEFORE page tables so that stack overflow
         * (which grows downward) goes into BSS rather than corrupting
         * page tables.
         */
        . = ALIGN(4096);
        . += 0x200000; /* 2MB boot stack for debug builds */
        __boot_stack = . - VMA_OFFSET; /* physical address (top of stack) */

        /* Initial boot page tables (physical addresses). */
        . = ALIGN(4096);
        __kernel_pgd_ttbr1 = . - VMA_OFFSET;
        . += 0x1000;
        __kernel_pud_ttbr1 = . - VMA_OFFSET;
        . += 0x1000;
        __kernel_pmd_ttbr1 = . - VMA_OFFSET;
        . += 4 * 512 * 8; /* 4 PUD entries * 512 PMD entries * 8 bytes */

        /* Identity-map page tables for boot (TTBR0). */
        __kernel_pgd_ttbr0 = . - VMA_OFFSET;
        . += 0x1000;
        __kernel_pud_ttbr0 = . - VMA_OFFSET;
        . += 0x1000;
        __kernel_pmd_ttbr0 = . - VMA_OFFSET;
        . += 4 * 512 * 8;

        . = ALIGN(4096);
        __kernel_image_end = . - VMA_OFFSET; /* physical address */
    }

    /DISCARD/ : {
        *(*.eh_frame);
        *(*.eh_frame_hdr);
    }
}
