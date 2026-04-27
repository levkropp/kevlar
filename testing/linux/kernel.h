/* SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause */
/* Kevlar kABI K7 compat: <linux/kernel.h> */
#ifndef _LINUX_KERNEL_H
#define _LINUX_KERNEL_H

#include <linux/types.h>
#include <linux/compiler.h>
#include <linux/stddef.h>
#include <linux/printk.h>

#define ARRAY_SIZE(arr) (sizeof(arr) / sizeof((arr)[0]))

#define container_of(ptr, type, member) \
    ((type *)((char *)(ptr) - offsetof(type, member)))

#define min(a, b)  ((a) < (b) ? (a) : (b))
#define max(a, b)  ((a) > (b) ? (a) : (b))
#define swap(a, b) do { typeof(a) __tmp = (a); (a) = (b); (b) = __tmp; } while (0)

#define BUG()       __builtin_trap()
#define BUG_ON(c)   do { if (unlikely(c)) BUG(); } while (0)
#define WARN_ON(c)  ({ int __c = !!(c); if (__c) pr_warn("WARN_ON: %s\n", #c); __c; })

/* Frequently used by Linux module authors. */
#define IS_ERR_VALUE(x) unlikely((unsigned long)(x) >= (unsigned long)-4095)
#define ERR_PTR(err)    ((void *)(long)(err))
#define PTR_ERR(p)      ((long)(p))
#define IS_ERR(p)       IS_ERR_VALUE((unsigned long)(p))

#endif /* _LINUX_KERNEL_H */
