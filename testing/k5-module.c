/* SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause */
/* Kevlar kABI K5 demo module.
 *
 * Allocates a DMA-coherent buffer, ioremap's its physical address
 * to get a second kernel mapping, then writes/reads via the MMIO
 * accessors and verifies both pointer-views see the same memory.
 */
#include "kevlar_kabi_k5.h"

MODULE_LICENSE("GPL");
MODULE_AUTHOR("Kevlar");
MODULE_DESCRIPTION("kABI K5 demo: ioremap + readl/writel + dma_alloc_coherent");

int init_module(void) {
    printk("[k5] init begin\n");

    dma_addr_t dma_pa = 0;
    void *buf = dma_alloc_coherent(NULL, 4096, &dma_pa, 0);
    if (!buf) {
        printk("[k5] dma_alloc_coherent failed\n");
        return -1;
    }
    printk("[k5] dma_alloc_coherent ok\n");

    void *io = ioremap(dma_pa, 4096);
    if (!io) {
        printk("[k5] ioremap failed\n");
        return -1;
    }
    printk("[k5] ioremap ok\n");

    /* Write via MMIO, read via direct DMA pointer. */
    writel(0xCAFEBABE, io);
    unsigned int v1 = *(volatile unsigned int *)buf;
    if (v1 != 0xCAFEBABE) {
        printk("[k5] writel/buf mismatch\n");
        return -1;
    }
    printk("[k5] writel ok (buf reads 0xCAFEBABE)\n");

    /* Reverse: write via DMA pointer, read via readl. */
    *(volatile unsigned int *)buf = 0xDEADBEEF;
    unsigned int v2 = readl(io);
    if (v2 != 0xDEADBEEF) {
        printk("[k5] readl mismatch\n");
        return -1;
    }
    printk("[k5] readl ok (io reads 0xDEADBEEF)\n");

    /* Sanity: virt_to_phys round-trips through phys_to_virt. */
    unsigned long long pa2 = virt_to_phys(buf);
    void *va2 = phys_to_virt(pa2);
    if (va2 != buf) {
        printk("[k5] phys/virt round-trip mismatch\n");
        return -1;
    }
    printk("[k5] phys/virt round-trip ok\n");

    iounmap(io);
    dma_free_coherent(NULL, 4096, buf, dma_pa);
    printk("[k5] init done\n");
    return 0;
}
