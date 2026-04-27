/* SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause */
/* Kevlar kABI K7 compat: <linux/types.h> — kernel typedefs.
 *
 * Linux defines u8/u16/u32/u64 + s8/.../s64 via either GCC builtins or
 * arch-specific includes.  Kevlar's K7 compat just declares them
 * directly using GCC __INT*_TYPE__ builtins (which are stable across
 * any modern GCC + clang).
 */
#ifndef _LINUX_TYPES_H
#define _LINUX_TYPES_H

typedef unsigned char        u8;
typedef unsigned short       u16;
typedef unsigned int         u32;
typedef unsigned long long   u64;
typedef signed char          s8;
typedef short                s16;
typedef int                  s32;
typedef long long            s64;

typedef unsigned char        __u8;
typedef unsigned short       __u16;
typedef unsigned int         __u32;
typedef unsigned long long   __u64;
typedef signed char          __s8;
typedef short                __s16;
typedef int                  __s32;
typedef long long            __s64;

typedef long                 ssize_t;
typedef unsigned long        size_t;
typedef long                 loff_t;
typedef unsigned int         dev_t;
typedef unsigned long long   dma_addr_t;
typedef unsigned long        phys_addr_t;

/* bool: include <stdbool.h> if you need it; some toolchains
 * predefine `bool` and rejecting a typedef-redefinition. */

#endif /* _LINUX_TYPES_H */
