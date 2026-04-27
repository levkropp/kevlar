/* SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause */
/* Userspace test binary for kABI K4 char-device validation.
 *
 * Boots as PID 1 (via kernel cmdline init=/usr/bin/test-kabi-userspace).
 * Opens /dev/k4-demo, reads bytes through the real syscall path, and
 * writes markers to stdout (which lands on serial).  The kernel-side
 * test then greps the serial log for the markers.
 */
#include <fcntl.h>
#include <unistd.h>
#include <string.h>

static void w(const char *s) {
    write(1, s, strlen(s));
}

int main(void) {
    w("USERSPACE: starting\n");

    int fd = open("/dev/k4-demo", O_RDONLY);
    if (fd < 0) {
        w("USERSPACE: open failed\n");
        return 1;
    }
    w("USERSPACE: open ok\n");

    char buf[64];
    ssize_t n = read(fd, buf, sizeof(buf) - 1);
    if (n < 0) {
        w("USERSPACE: read failed\n");
        close(fd);
        return 1;
    }
    buf[n] = 0;
    w("USERSPACE: read=");
    write(1, buf, n);

    close(fd);
    w("USERSPACE: done\n");
    return 0;
}
