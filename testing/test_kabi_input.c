/* SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause */
/* Userspace test for kABI K24: open /dev/input/event0 and query
 * the device name via EVIOCGNAME.  Validates the full pipe from
 * K23's virtio_input.probe() through K24's
 * input_register_device through Kevlar's existing evdev
 * infrastructure (kernel/fs/devfs/evdev.rs).
 *
 * Boots via kernel cmdline INIT_SCRIPT=/usr/bin/test-kabi-input.
 */
#include <fcntl.h>
#include <unistd.h>
#include <string.h>
#include <stddef.h>
#include <stdio.h>
#include <sys/ioctl.h>

/* Define _IOC encoding inline to avoid host-vs-target header
 * differences when this file is built by aarch64-linux-musl-gcc.
 * dir=2 (READ), type='E', nr=0x06.  Matches Linux's
 * <asm-generic/ioctl.h>. */
#define KABI_IOC_READ  2u
#define KABI_IOC(dir, type, nr, size) \
    (((dir) << 30) | ((type) << 8) | (nr) | ((size) << 16))
#define EVIOCGNAME(len) KABI_IOC(KABI_IOC_READ, 'E', 0x06, (len))

static void w(const char *s) {
    write(1, s, strlen(s));
}

int main(void) {
    w("USERSPACE-INPUT: starting\n");

    /* Scan event0..event3 for an evdev that EVIOCGNAME's as
     * "kabi-virtio-input" (the name K24's input_register_device
     * uses).  Native virtio_input devices may already occupy the
     * lower indices; the kABI registration lands at the next
     * free slot.  O_NONBLOCK so a future read() wouldn't block
     * forever. */
    int found_open = 0;
    int found_kabi = 0;
    char path[32];
    for (int i = 0; i < 4; i++) {
        snprintf(path, sizeof(path), "/dev/input/event%d", i);
        int fd = open(path, O_RDONLY | O_NONBLOCK);
        if (fd < 0) continue;
        if (!found_open) {
            w("USERSPACE-INPUT: open ok\n");
            found_open = 1;
        }

        char name[64] = {0};
        if (ioctl(fd, EVIOCGNAME(sizeof(name)), name) >= 0) {
            char line[160];
            int n = snprintf(line, sizeof(line),
                             "USERSPACE-INPUT: event%d name=%s\n",
                             i, name);
            if (n > 0) write(1, line, n);
            if (strcmp(name, "kabi-virtio-input") == 0) {
                w("USERSPACE-INPUT: name=kabi-virtio-input\n");
                found_kabi = 1;
            }
        }
        close(fd);
    }

    if (!found_open) {
        w("USERSPACE-INPUT: open failed\n");
        return 1;
    }
    if (!found_kabi) {
        w("USERSPACE-INPUT: kabi device not found\n");
    }

    w("USERSPACE-INPUT: done\n");
    return 0;
}
