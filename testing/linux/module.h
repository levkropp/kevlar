/* SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause */
/* Kevlar kABI K7 compat: <linux/module.h>
 *
 * `MODULE_LICENSE` / `MODULE_AUTHOR` / etc. emit NUL-terminated
 * "key=value" strings into `.modinfo` — exactly what Linux's
 * `__MODULE_INFO()` macro does, exactly what Kevlar's K2 loader
 * parses on module load.
 */
#ifndef _LINUX_MODULE_H
#define _LINUX_MODULE_H

#include <linux/init.h>
#include <linux/types.h>
#include <linux/kernel.h>

/* `struct module` is opaque from a hello-world module's
 * perspective.  Real Linux's struct is hundreds of bytes; K7
 * exposes it as an opaque tag.  K8+ adds field layout when a
 * module first reads through it. */
struct module;

/* THIS_MODULE expands to `&__this_module`; the linker normally
 * fills `__this_module` from `.gnu.linkonce.this_module`.  K7
 * stubs to NULL — modules that pass `THIS_MODULE` as an argument
 * (most commonly `.owner = THIS_MODULE` in struct
 * file_operations) get NULL, which our shims accept. */
#define THIS_MODULE  ((struct module *)0)

/* __MODULE_INFO(tag, name, value):
 *
 * Emit a `tag=value` NUL-terminated string into `.modinfo`.
 * The unique-name machinery uses __LINE__ since GCC's
 * __COUNTER__ may not be available in all toolchain configs;
 * one MODULE_INFO per source line is fine.
 */
#define __MODULE_INFO_PASTE(a, b)  a##b
#define __MODULE_INFO_UNIQUE(line) __MODULE_INFO_PASTE(__module_info_, line)
#define MODULE_INFO(tag, val)                                            \
    static const char __MODULE_INFO_UNIQUE(__LINE__)[]                   \
        __attribute__((section(".modinfo"), aligned(1), used)) =         \
        #tag "=" val

#define MODULE_LICENSE(s)         MODULE_INFO(license, s)
#define MODULE_AUTHOR(s)          MODULE_INFO(author, s)
#define MODULE_DESCRIPTION(s)     MODULE_INFO(description, s)
#define MODULE_VERSION(s)         MODULE_INFO(version, s)
#define MODULE_ALIAS(s)           MODULE_INFO(alias, s)
#define MODULE_DEVICE_TABLE(_, _2)  /* nothing — DT/PCI table registration */

#endif /* _LINUX_MODULE_H */
