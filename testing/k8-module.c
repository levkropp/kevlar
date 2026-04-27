// SPDX-License-Identifier: GPL-2.0
/*
 * Kevlar kABI K8 demo: hello-world compiled against Linux 7.0's
 * actual UAPI headers — sourced from Ubuntu 26.04's `linux-headers-
 * 7.0.0-14-generic` package, merged into build/linux-src/.
 *
 * This .c file is identical in shape to a real Linux out-of-tree
 * module's hello-world.  When K8 succeeds, the C preprocessor walks
 * Ubuntu's prebuilt Linux 7.0 tree, not Kevlar's compat tree.
 */
#include <linux/init.h>
#include <linux/module.h>
#include <linux/kernel.h>

MODULE_LICENSE("GPL");
MODULE_AUTHOR("Kevlar");
MODULE_DESCRIPTION("kABI K8: Linux 7.0 / Ubuntu 26.04 headers hello-world");
MODULE_VERSION("0.1");

static int __init k8_init(void)
{
	pr_info("k8: hello from real Linux 7.0 headers (Ubuntu 26.04)\n");
	pr_info("k8: built against build/linux-src/include/\n");
	return 0;
}

static void __exit k8_exit(void)
{
	pr_info("k8: goodbye\n");
}

module_init(k8_init);
module_exit(k8_exit);
