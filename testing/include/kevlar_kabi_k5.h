/* SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause */
/* Kevlar kABI K5 — module-side header for K5 demo .ko files.
 *
 * Adds Linux's MMIO + DMA primitives: ioremap, readb/writel,
 * dma_alloc_coherent, dma_map_single.
 */
#ifndef KEVLAR_KABI_K5_H
#define KEVLAR_KABI_K5_H

#include "kevlar_kabi_k4.h"

#define DMA_BIDIRECTIONAL  0
#define DMA_TO_DEVICE      1
#define DMA_FROM_DEVICE    2

typedef unsigned long long dma_addr_t;
typedef unsigned long      phys_addr_t;

/* ── MMIO accessors ─────────────────────────────────────────── */
extern unsigned char        readb(const void *addr);
extern unsigned short       readw(const void *addr);
extern unsigned int         readl(const void *addr);
extern unsigned long long   readq(const void *addr);
extern void writeb(unsigned char val, void *addr);
extern void writew(unsigned short val, void *addr);
extern void writel(unsigned int val, void *addr);
extern void writeq(unsigned long long val, void *addr);

/* ── ioremap ────────────────────────────────────────────────── */
extern void *ioremap(unsigned long long phys, size_t size);
extern void *ioremap_wc(unsigned long long phys, size_t size);
extern void *ioremap_nocache(unsigned long long phys, size_t size);
extern void *ioremap_cache(unsigned long long phys, size_t size);
extern void  iounmap(void *addr);

/* ── DMA ────────────────────────────────────────────────────── */
extern void *dma_alloc_coherent(struct device *dev, size_t size,
                                dma_addr_t *dma_handle, unsigned int gfp);
extern void  dma_free_coherent(struct device *dev, size_t size,
                               void *vaddr, dma_addr_t dma_handle);
extern dma_addr_t dma_map_single(struct device *dev, void *ptr,
                                 size_t size, int dir);
extern void dma_unmap_single(struct device *dev, dma_addr_t addr,
                             size_t size, int dir);

/* ── physaddr ↔ virtaddr ────────────────────────────────────── */
extern unsigned long long virt_to_phys(void *va);
extern void *phys_to_virt(unsigned long long pa);

#endif /* KEVLAR_KABI_K5_H */
