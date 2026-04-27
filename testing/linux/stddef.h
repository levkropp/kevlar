/* SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause */
/* Kevlar kABI K7 compat: <linux/stddef.h> */
#ifndef _LINUX_STDDEF_H
#define _LINUX_STDDEF_H

#ifndef NULL
#define NULL ((void *)0)
#endif

#ifndef offsetof
#define offsetof(TYPE, MEMBER) __builtin_offsetof(TYPE, MEMBER)
#endif

#endif /* _LINUX_STDDEF_H */
