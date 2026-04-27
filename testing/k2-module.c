/* SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause */
/* Kevlar kABI K2 demo module.
 *
 * Exercises every K2 primitive in concert: kmalloc/vmalloc, wait
 * queues, completions, work_struct + workqueue, msleep.  Init runs
 * synchronously in the kernel's boot context (called from main.rs);
 * the work_struct callback runs on the kABI worker kthread; init
 * sleeps on the wait queue until the worker wakes it.
 */
#include "kevlar_kabi_k2.h"

MODULE_LICENSE("GPL");
MODULE_AUTHOR("Kevlar");
MODULE_DESCRIPTION("kABI K2 demo: alloc + wait + work + completion");

static struct wait_queue_head wq;
static struct completion done;
static struct work_struct work;
static int condition;

static void k2_work_handler(struct work_struct *w) {
    printk("[k2] work handler running on worker thread\n");
    void *vbuf = vmalloc(8192);
    if (!vbuf) {
        printk("[k2] vmalloc failed\n");
        return;
    }
    msleep(50);
    condition = 1;
    wake_up(&wq);
    complete(&done);
    vfree(vbuf);
    printk("[k2] work handler done\n");
}

static int k2_cond(void *arg) {
    return condition;
}

int init_module(void) {
    printk("[k2] init begin\n");

    /* Allocator smoke test. */
    void *p = kmalloc(256, 0);
    if (!p) {
        printk("[k2] kmalloc failed\n");
        return -1;
    }
    void *zp = kzalloc(64, 0);
    if (!zp) {
        printk("[k2] kzalloc failed\n");
        kfree(p);
        return -1;
    }

    /* Initialize wait + completion + work. */
    init_waitqueue_head(&wq);
    init_completion(&done);
    kabi_init_work(&work, k2_work_handler);

    printk("[k2] scheduling work\n");
    schedule_work(&work);

    /* Sleep until the worker sets condition=1 and wakes us. */
    kabi_wait_event(&wq, k2_cond, NULL);
    printk("[k2] woken by worker\n");

    /* Belt-and-suspenders: also wait on the completion. */
    wait_for_completion(&done);
    printk("[k2] completion observed\n");

    /* Ensure the work has fully drained before tearing down. */
    flush_work(&work);

    /* Cleanup. */
    kfree(p);
    kfree(zp);
    destroy_waitqueue_head(&wq);
    destroy_completion(&done);

    printk("[k2] init done\n");
    return 0;
}
