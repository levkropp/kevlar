/* SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause */
/* Kevlar kABI K7 compat: <linux/compiler.h> */
#ifndef _LINUX_COMPILER_H
#define _LINUX_COMPILER_H

#define __must_check        __attribute__((warn_unused_result))
#define __maybe_unused      __attribute__((unused))
#define __used              __attribute__((used))
#define __aligned(x)        __attribute__((aligned(x)))
#define __packed            __attribute__((packed))
#define __pure              __attribute__((pure))
#define __noreturn          __attribute__((noreturn))
#define __weak              __attribute__((weak))
#define __always_inline     inline __attribute__((always_inline))
#define noinline            __attribute__((noinline))

#define likely(x)           __builtin_expect(!!(x), 1)
#define unlikely(x)         __builtin_expect(!!(x), 0)

/* Markers Linux uses to flag user/kernel pointer separation.  K7
 * accepts both as no-ops; sparse static analysis honors them but
 * we don't run sparse. */
#define __user
#define __kernel
#define __iomem
#define __force
#define __rcu
#define __percpu

/* READ_ONCE/WRITE_ONCE: real Linux uses these for tearing-safe
 * accesses to volatile-ish fields.  K7 collapses to volatile load/
 * store. */
#define READ_ONCE(x)        (*(volatile typeof(x) *)&(x))
#define WRITE_ONCE(x, val)  (*(volatile typeof(x) *)&(x) = (val))

#endif /* _LINUX_COMPILER_H */
