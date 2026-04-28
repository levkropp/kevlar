/* SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause */
/* Userspace test for kABI K22: open /dev/dri/card0 and issue
 * DRM_IOCTL_VERSION through the real syscall path.  Validates
 * Kevlar's K4 char-device dispatch + K20 fops adapter + K21
 * drm_ioctl handler from a real PID-1 userspace process.
 *
 * Boots via kernel cmdline INIT_SCRIPT=/usr/bin/test-kabi-drm.
 * Output goes to stdout (which lands on Kevlar's serial console);
 * the kernel-side test target greps the serial log for markers.
 */
#include <fcntl.h>
#include <unistd.h>
#include <string.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <sys/ioctl.h>

/* Mirror of Linux's struct drm_version.  Same layout K21's
 * `DrmVersion` mirrors on the kernel side.  3×i32 + 4 bytes
 * implicit pad + 8×8-byte fields = 64 bytes on 64-bit. */
struct drm_version {
    int version_major;
    int version_minor;
    int version_patchlevel;
    size_t name_len;
    char *name;
    size_t date_len;
    char *date;
    size_t desc_len;
    char *desc;
};

#define DRM_IOCTL_VERSION _IOWR('d', 0x00, struct drm_version)

static void w(const char *s) {
    write(1, s, strlen(s));
}

int main(void) {
    w("USERSPACE-DRM: starting\n");

    int fd = open("/dev/dri/card0", O_RDWR);
    if (fd < 0) {
        w("USERSPACE-DRM: open failed\n");
        return 1;
    }
    w("USERSPACE-DRM: open ok\n");

    char name_buf[64], date_buf[64], desc_buf[64];
    memset(name_buf, 0, sizeof(name_buf));
    memset(date_buf, 0, sizeof(date_buf));
    memset(desc_buf, 0, sizeof(desc_buf));

    struct drm_version v;
    memset(&v, 0, sizeof(v));
    v.name = name_buf;  v.name_len = sizeof(name_buf);
    v.date = date_buf;  v.date_len = sizeof(date_buf);
    v.desc = desc_buf;  v.desc_len = sizeof(desc_buf);

    int rc = ioctl(fd, DRM_IOCTL_VERSION, &v);
    if (rc < 0) {
        w("USERSPACE-DRM: ioctl failed\n");
        return 2;
    }

    char line[256];
    int n = snprintf(line, sizeof(line),
        "USERSPACE-DRM: name=%.*s version=%d.%d.%d\n",
        (int)v.name_len, name_buf,
        v.version_major, v.version_minor, v.version_patchlevel);
    if (n > 0) write(1, line, n);

    close(fd);
    w("USERSPACE-DRM: done\n");
    return 0;
}
