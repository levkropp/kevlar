/* SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause */
/* Kevlar kABI K6 demo module.
 *
 * Exercises the new variadic printk format-string parser.
 * Each printk call uses a different conversion / flag, and the
 * test target greps the serial log for the expected outputs.
 */
#include "kevlar_kabi_k5.h"

MODULE_LICENSE("GPL");
MODULE_AUTHOR("Kevlar");
MODULE_DESCRIPTION("kABI K6 demo: variadic printk + format strings");

int init_module(void) {
    printk("[k6] init begin\n");
    printk("[k6] decimal: %d\n", 42);
    printk("[k6] negative: %d\n", -7);
    printk("[k6] unsigned: %u\n", 4294967290U);
    printk("[k6] hex: %x\n", 0xcafebabe);
    printk("[k6] HEX: %X\n", 0xdeadbeef);
    printk("[k6] string: %s\n", "world");
    printk("[k6] char: %c\n", 'A');
    printk("[k6] pointer: %p\n", (void *)0xffff000040000000UL);
    printk("[k6] padded: %05d\n", 42);
    printk("[k6] mixed: %s = %d (0x%x)\n", "answer", 42, 42);
    printk("[k6] percent: 100%%\n");
    printk("[k6] init done\n");
    return 0;
}
