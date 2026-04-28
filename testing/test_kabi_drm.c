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

/* K25: DRM_IOCTL_MODE_GETRESOURCES = _IOWR('d', 0xA0, struct
 * drm_mode_card_res). */
struct drm_mode_card_res {
    uint64_t fb_id_ptr;
    uint64_t crtc_id_ptr;
    uint64_t connector_id_ptr;
    uint64_t encoder_id_ptr;
    uint32_t count_fbs;
    uint32_t count_crtcs;
    uint32_t count_connectors;
    uint32_t count_encoders;
    uint32_t min_width;
    uint32_t max_width;
    uint32_t min_height;
    uint32_t max_height;
};
#define DRM_IOCTL_MODE_GETRESOURCES _IOWR('d', 0xA0, struct drm_mode_card_res)

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

    /* K25: DRM_IOCTL_MODE_GETRESOURCES — counts + ID arrays. */
    uint32_t crtc_ids[4] = {0};
    uint32_t conn_ids[4] = {0};
    uint32_t enc_ids[4] = {0};
    struct drm_mode_card_res res;
    memset(&res, 0, sizeof(res));
    res.crtc_id_ptr = (uint64_t)(uintptr_t)crtc_ids;
    res.connector_id_ptr = (uint64_t)(uintptr_t)conn_ids;
    res.encoder_id_ptr = (uint64_t)(uintptr_t)enc_ids;
    res.count_crtcs = 4;
    res.count_connectors = 4;
    res.count_encoders = 4;

    if (ioctl(fd, DRM_IOCTL_MODE_GETRESOURCES, &res) < 0) {
        w("USERSPACE-DRM: MODE_GETRESOURCES failed\n");
    } else {
        n = snprintf(line, sizeof(line),
            "USERSPACE-DRM: getres crtcs=%u connectors=%u encoders=%u "
            "geom=%ux%u-%ux%u crtc0=0x%x conn0=0x%x enc0=0x%x\n",
            res.count_crtcs, res.count_connectors, res.count_encoders,
            res.min_width, res.min_height, res.max_width, res.max_height,
            crtc_ids[0], conn_ids[0], enc_ids[0]);
        if (n > 0) write(1, line, n);
    }

    close(fd);
    w("USERSPACE-DRM: done\n");
    return 0;
}
