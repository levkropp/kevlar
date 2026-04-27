/* SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause */
/* Kevlar kABI K3 demo module.
 *
 * Registers a platform_device + a matching platform_driver from
 * the same module and watches the kernel-side bus call probe.
 * No real hardware — purely device-model bookkeeping.
 */
#include "kevlar_kabi_k3.h"

MODULE_LICENSE("GPL");
MODULE_AUTHOR("Kevlar");
MODULE_DESCRIPTION("kABI K3 demo: platform_device + platform_driver bind");

static int probe_called;

static int k3_probe(struct platform_device *pdev) {
    probe_called = 1;
    printk("[k3] probe called\n");
    return 0;
}

static int k3_remove(struct platform_device *pdev) {
    printk("[k3] remove called\n");
    return 0;
}

static struct platform_device k3_pdev = {
    .name = "k3-demo",
    .id   = 0,
};

static struct platform_driver k3_pdrv = {
    .probe  = k3_probe,
    .remove = k3_remove,
    .driver = { .name = "k3-demo" },
};

int init_module(void) {
    printk("[k3] init begin\n");

    int rc = platform_device_register(&k3_pdev);
    if (rc != 0) {
        printk("[k3] platform_device_register failed\n");
        return rc;
    }
    printk("[k3] platform_device_register ok\n");

    rc = platform_driver_register(&k3_pdrv);
    if (rc != 0) {
        printk("[k3] platform_driver_register failed\n");
        return rc;
    }
    printk("[k3] platform_driver_register ok\n");

    if (probe_called) {
        printk("[k3] probe_called observed\n");
    } else {
        printk("[k3] probe NOT called — bind failed\n");
        return -1;
    }

    printk("[k3] init done\n");
    return 0;
}
