/* SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause */
/* Kevlar kABI K4 demo module.
 *
 * register_chrdev("k4-demo", &fops) → /dev/k4-demo
 * Module's read fop returns "hello from k4\n".
 */
#include "kevlar_kabi_k4.h"

MODULE_LICENSE("GPL");
MODULE_AUTHOR("Kevlar");
MODULE_DESCRIPTION("kABI K4 demo: file_operations + char-device");

static const char K4_MSG[] = "hello from k4\n";
static const size_t K4_MSG_LEN = 14;

static int k4_open(struct inode *inode, struct file *filp) {
    printk("[k4] open called\n");
    return 0;
}

static int k4_release(struct inode *inode, struct file *filp) {
    printk("[k4] release called\n");
    return 0;
}

static ssize_t k4_read(struct file *filp, char *buf,
                       size_t count, loff_t *pos) {
    if (*pos >= (loff_t)K4_MSG_LEN) return 0;
    size_t remaining = K4_MSG_LEN - (size_t)*pos;
    size_t n = count < remaining ? count : remaining;
    if (copy_to_user(buf, K4_MSG + *pos, n) != 0) return -14; /* -EFAULT */
    *pos += (loff_t)n;
    return (ssize_t)n;
}

static struct file_operations k4_fops = {
    .open    = k4_open,
    .release = k4_release,
    .read    = k4_read,
};

int init_module(void) {
    printk("[k4] init begin\n");
    int major = register_chrdev(0, "k4-demo", &k4_fops);
    if (major < 0) {
        printk("[k4] register_chrdev failed\n");
        return major;
    }
    printk("[k4] register_chrdev ok\n");
    printk("[k4] init done\n");
    return 0;
}
