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
#include <sys/mman.h>

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

/* K26: per-ID DRM ioctl structs. */
struct drm_mode_modeinfo {
    uint32_t clock;
    uint16_t hdisplay, hsync_start, hsync_end, htotal, hskew;
    uint16_t vdisplay, vsync_start, vsync_end, vtotal, vscan;
    uint32_t vrefresh;
    uint32_t flags;
    uint32_t type;
    char     name[32];
};

struct drm_mode_crtc {
    uint64_t set_connectors_ptr;
    uint32_t count_connectors;
    uint32_t crtc_id;
    uint32_t fb_id;
    uint32_t x, y;
    uint32_t gamma_size;
    uint32_t mode_valid;
    struct drm_mode_modeinfo mode;
};

struct drm_mode_get_encoder {
    uint32_t encoder_id;
    uint32_t encoder_type;
    uint32_t crtc_id;
    uint32_t possible_crtcs;
    uint32_t possible_clones;
};

struct drm_mode_get_connector {
    uint64_t encoders_ptr;
    uint64_t modes_ptr;
    uint64_t props_ptr;
    uint64_t prop_values_ptr;
    uint32_t count_modes;
    uint32_t count_props;
    uint32_t count_encoders;
    uint32_t encoder_id;
    uint32_t connector_id;
    uint32_t connector_type;
    uint32_t connector_type_id;
    uint32_t connection;
    uint32_t mm_width;
    uint32_t mm_height;
    uint32_t subpixel;
    uint32_t pad;
};

#define DRM_IOCTL_MODE_GETCRTC      _IOWR('d', 0xA1, struct drm_mode_crtc)
#define DRM_IOCTL_MODE_GETENCODER   _IOWR('d', 0xA6, struct drm_mode_get_encoder)
#define DRM_IOCTL_MODE_GETCONNECTOR _IOWR('d', 0xA7, struct drm_mode_get_connector)

/* K27: framebuffer + setcrtc structs. */
struct drm_mode_fb_cmd2 {
    uint32_t fb_id;
    uint32_t width;
    uint32_t height;
    uint32_t pixel_format;
    uint32_t flags;
    uint32_t handles[4];
    uint32_t pitches[4];
    uint32_t offsets[4];
    uint64_t modifier[4];
};

#define DRM_IOCTL_MODE_SETCRTC _IOWR('d', 0xA2, struct drm_mode_crtc)
#define DRM_IOCTL_MODE_ADDFB2  _IOWR('d', 0xB8, struct drm_mode_fb_cmd2)

/* K28: DUMB buffer + mmap structs. */
struct drm_mode_create_dumb {
    uint32_t height;
    uint32_t width;
    uint32_t bpp;
    uint32_t flags;
    uint32_t handle;
    uint32_t pitch;
    uint64_t size;
};

struct drm_mode_map_dumb {
    uint32_t handle;
    uint32_t pad;
    uint64_t offset;
};

#define DRM_IOCTL_MODE_CREATE_DUMB _IOWR('d', 0xB2, struct drm_mode_create_dumb)
#define DRM_IOCTL_MODE_MAP_DUMB    _IOWR('d', 0xB3, struct drm_mode_map_dumb)

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

    /* K26: per-ID walk. */
    struct drm_mode_crtc crtc;
    memset(&crtc, 0, sizeof(crtc));
    crtc.crtc_id = crtc_ids[0];
    if (ioctl(fd, DRM_IOCTL_MODE_GETCRTC, &crtc) < 0) {
        w("USERSPACE-DRM: MODE_GETCRTC failed\n");
    } else {
        n = snprintf(line, sizeof(line),
            "USERSPACE-DRM: getcrtc id=0x%x mode_valid=%u\n",
            crtc.crtc_id, crtc.mode_valid);
        if (n > 0) write(1, line, n);
    }

    struct drm_mode_get_encoder enc;
    memset(&enc, 0, sizeof(enc));
    enc.encoder_id = enc_ids[0];
    if (ioctl(fd, DRM_IOCTL_MODE_GETENCODER, &enc) < 0) {
        w("USERSPACE-DRM: MODE_GETENCODER failed\n");
    } else {
        n = snprintf(line, sizeof(line),
            "USERSPACE-DRM: getenc id=0x%x type=%u crtc=0x%x\n",
            enc.encoder_id, enc.encoder_type, enc.crtc_id);
        if (n > 0) write(1, line, n);
    }

    struct drm_mode_get_connector conn;
    memset(&conn, 0, sizeof(conn));
    conn.connector_id = conn_ids[0];
    if (ioctl(fd, DRM_IOCTL_MODE_GETCONNECTOR, &conn) < 0) {
        w("USERSPACE-DRM: MODE_GETCONNECTOR failed\n");
    } else {
        n = snprintf(line, sizeof(line),
            "USERSPACE-DRM: getconn id=0x%x type=%u connection=%u enc=0x%x\n",
            conn.connector_id, conn.connector_type, conn.connection, conn.encoder_id);
        if (n > 0) write(1, line, n);
    }

    /* K27a: re-query connector with modes_ptr to fetch the
     * advertised mode list. */
    struct drm_mode_modeinfo mode_buf[4];
    memset(mode_buf, 0, sizeof(mode_buf));
    struct drm_mode_get_connector conn2;
    memset(&conn2, 0, sizeof(conn2));
    conn2.connector_id = conn_ids[0];
    conn2.count_modes = 4;
    conn2.modes_ptr = (uint64_t)(uintptr_t)mode_buf;
    if (ioctl(fd, DRM_IOCTL_MODE_GETCONNECTOR, &conn2) < 0) {
        w("USERSPACE-DRM: MODE_GETCONNECTOR (modes) failed\n");
    } else {
        n = snprintf(line, sizeof(line),
            "USERSPACE-DRM: getconn.modes count=%u mode0=%s %ux%u@%uHz\n",
            conn2.count_modes,
            mode_buf[0].name,
            mode_buf[0].hdisplay, mode_buf[0].vdisplay,
            mode_buf[0].vrefresh);
        if (n > 0) write(1, line, n);
    }

    /* K27b: ADDFB2 — get a fb_id. */
    struct drm_mode_fb_cmd2 fbcmd;
    memset(&fbcmd, 0, sizeof(fbcmd));
    fbcmd.width = 1024;
    fbcmd.height = 768;
    fbcmd.pixel_format = 0x34325258; /* 'XR24' = DRM_FORMAT_XRGB8888 */
    if (ioctl(fd, DRM_IOCTL_MODE_ADDFB2, &fbcmd) < 0) {
        w("USERSPACE-DRM: MODE_ADDFB2 failed\n");
    } else {
        n = snprintf(line, sizeof(line),
            "USERSPACE-DRM: addfb2 fb_id=%u\n", fbcmd.fb_id);
        if (n > 0) write(1, line, n);
    }

    /* K27c: SETCRTC — attach fb + mode. */
    struct drm_mode_crtc setcrtc;
    memset(&setcrtc, 0, sizeof(setcrtc));
    setcrtc.crtc_id = crtc_ids[0];
    setcrtc.fb_id = fbcmd.fb_id;
    setcrtc.mode_valid = 1;
    setcrtc.mode = mode_buf[0];
    if (ioctl(fd, DRM_IOCTL_MODE_SETCRTC, &setcrtc) < 0) {
        w("USERSPACE-DRM: MODE_SETCRTC failed\n");
    } else {
        w("USERSPACE-DRM: setcrtc rc=0\n");
    }

    /* K28: DUMB buffer creation + mmap + draw + read-back. */
    struct drm_mode_create_dumb cdumb;
    memset(&cdumb, 0, sizeof(cdumb));
    cdumb.width = 1024;
    cdumb.height = 768;
    cdumb.bpp = 32;
    if (ioctl(fd, DRM_IOCTL_MODE_CREATE_DUMB, &cdumb) < 0) {
        w("USERSPACE-DRM: CREATE_DUMB failed\n");
    } else {
        n = snprintf(line, sizeof(line),
            "USERSPACE-DRM: dumb handle=%u pitch=%u size=%llu\n",
            cdumb.handle, cdumb.pitch, (unsigned long long)cdumb.size);
        if (n > 0) write(1, line, n);

        struct drm_mode_map_dumb mdumb;
        memset(&mdumb, 0, sizeof(mdumb));
        mdumb.handle = cdumb.handle;
        if (ioctl(fd, DRM_IOCTL_MODE_MAP_DUMB, &mdumb) < 0) {
            w("USERSPACE-DRM: MAP_DUMB failed\n");
        } else {
            n = snprintf(line, sizeof(line),
                "USERSPACE-DRM: mapdumb offset=0x%llx\n",
                (unsigned long long)mdumb.offset);
            if (n > 0) write(1, line, n);

            void *ptr = mmap(NULL, (size_t)cdumb.size,
                             PROT_READ | PROT_WRITE, MAP_SHARED,
                             fd, (off_t)mdumb.offset);
            if (ptr == MAP_FAILED) {
                w("USERSPACE-DRM: mmap failed\n");
            } else {
                volatile uint32_t *p = (volatile uint32_t *)ptr;
                p[0] = 0xCAFEF00Du;
                p[1] = 0xDEADBEEFu;
                uint32_t v0 = p[0];
                uint32_t v1 = p[1];
                n = snprintf(line, sizeof(line),
                    "USERSPACE-DRM: drew pattern[0]=0x%x [1]=0x%x\n",
                    v0, v1);
                if (n > 0) write(1, line, n);

                /* K29: paint a red 100x100 square at (10, 10).
                 * If QEMU has -device ramfb attached, this is
                 * visible in the host display.  Test verifies
                 * round-trip read of one interior pixel. */
                const uint32_t RED = 0x00FF0000u;
                for (int y = 10; y < 110; y++) {
                    uint32_t *row = (uint32_t *)((uint8_t *)ptr
                                    + y * cdumb.pitch);
                    for (int x = 10; x < 110; x++) {
                        row[x] = RED;
                    }
                }
                uint32_t *row50 = (uint32_t *)((uint8_t *)ptr
                                  + 50 * cdumb.pitch);
                n = snprintf(line, sizeof(line),
                    "USERSPACE-DRM: red square pixel(50,50)=0x%x\n",
                    row50[50]);
                if (n > 0) write(1, line, n);

                munmap(ptr, (size_t)cdumb.size);
            }
        }

        /* Re-issue ADDFB2 with the real handle this time. */
        struct drm_mode_fb_cmd2 fbcmd2;
        memset(&fbcmd2, 0, sizeof(fbcmd2));
        fbcmd2.width = cdumb.width;
        fbcmd2.height = cdumb.height;
        fbcmd2.pixel_format = 0x34325258;
        fbcmd2.handles[0] = cdumb.handle;
        fbcmd2.pitches[0] = cdumb.pitch;
        if (ioctl(fd, DRM_IOCTL_MODE_ADDFB2, &fbcmd2) < 0) {
            w("USERSPACE-DRM: ADDFB2(handle) failed\n");
        } else {
            n = snprintf(line, sizeof(line),
                "USERSPACE-DRM: addfb2(handle) fb_id=%u\n", fbcmd2.fb_id);
            if (n > 0) write(1, line, n);
        }
    }

    close(fd);
    w("USERSPACE-DRM: done\n");

    /* Hold the test process alive for a moment so an external
     * tool (qemu monitor screendump) has time to capture the
     * framebuffer.  Doesn't affect regression timing — the
     * markers are already printed.  Userspace gets 3 seconds. */
    sleep(3);
    return 0;
}
