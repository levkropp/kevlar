// SPDX-License-Identifier: GPL-2.0
/*
 * Kevlar kABI K7 demo: a Linux-source-shape "hello world" module.
 *
 * No `kevlar_kabi_*` references, no Kevlar-specific includes —
 * exactly the shape of every Linux 6.12 hello-world tutorial.
 * Compiles against the `testing/linux/` compat headers; binary
 * loads through Kevlar's K1 loader because `module_init(fn)`
 * aliases an `init_module` symbol to fn.
 */

#include <linux/init.h>
#include <linux/module.h>
#include <linux/kernel.h>

MODULE_LICENSE("GPL");
MODULE_AUTHOR("Kevlar");
MODULE_DESCRIPTION("kABI K7: Linux-source-shape hello-world module");
MODULE_VERSION("0.1");

static int __init k7_init(void)
{
	pr_info("k7: hello from a Linux-shape module v%d.%d\n", 1, 0);
	pr_info("k7: KERN_INFO + variadic printk works\n");
	return 0;
}

static void __exit k7_exit(void)
{
	pr_info("k7: goodbye\n");
}

module_init(k7_init);
module_exit(k7_exit);
