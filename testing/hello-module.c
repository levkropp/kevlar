/* Kevlar kABI K1 demo module.
 *
 * Built as ET_REL with `aarch64-linux-musl-gcc -c -fno-pic
 * -mcmodel=tiny -ffreestanding -nostdlib`.  Loaded by
 * `kernel/kabi/loader.rs::load_module()` at boot from
 * /lib/modules/hello.ko.
 *
 * Resolves `printk` against the kernel's exported-symbol table
 * (.ksymtab); see kernel/kabi/exports.rs + kernel/kabi/printk.rs.
 *
 * The "my_init" symbol is the entry point (K1's hardcoded entry
 * name; K2 will honor Linux's module_init() macro convention).
 */

extern void printk(const char *fmt);

int my_init(void) {
    printk("hello from module!\n");
    return 0;
}
