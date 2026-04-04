// Mimics exactly what Xorg's fbdevHWProbe + fbdevHWInit do.
// Run from INSIDE a chroot with /dev/fb0 available.
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>

#define FBIOGET_VSCREENINFO 0x4600
#define FBIOGET_FSCREENINFO 0x4602

int main(void) {
    const char *dev = "/dev/fb0";
    struct stat st;

    printf("fb0_probe: testing %s\n", dev);

    // Step 1: stat (what Xorg does first)
    if (stat(dev, &st) < 0) {
        printf("FAIL: stat(%s): %s (errno=%d)\n", dev, strerror(errno), errno);
        return 1;
    }
    printf("  stat: mode=%o, rdev=%lu (major=%lu minor=%lu)\n",
           st.st_mode, (unsigned long)st.st_rdev,
           (unsigned long)((st.st_rdev >> 8) & 0xfff),
           (unsigned long)(st.st_rdev & 0xff));
    if (!(st.st_mode & 0020000)) { // S_IFCHR = 020000
        printf("FAIL: not a character device (mode=%o)\n", st.st_mode);
        return 1;
    }
    printf("  stat: IS character device\n");

    // Step 2: open O_RDONLY (fbdevHWProbe)
    int fd = open(dev, O_RDONLY);
    if (fd < 0) {
        printf("FAIL: open O_RDONLY: %s (errno=%d)\n", strerror(errno), errno);
        return 1;
    }
    printf("  open O_RDONLY: fd=%d OK\n", fd);
    close(fd);

    // Step 3: open O_RDWR (fbdevHWInit)
    fd = open(dev, O_RDWR);
    if (fd < 0) {
        printf("FAIL: open O_RDWR: %s (errno=%d)\n", strerror(errno), errno);
        return 1;
    }
    printf("  open O_RDWR: fd=%d OK\n", fd);

    // Step 4: FBIOGET_FSCREENINFO (what fbdevHWInit does)
    unsigned char finfo[68];
    memset(finfo, 0, sizeof(finfo));
    if (ioctl(fd, FBIOGET_FSCREENINFO, finfo) < 0) {
        printf("FAIL: FBIOGET_FSCREENINFO: %s (errno=%d)\n", strerror(errno), errno);
        close(fd);
        return 1;
    }
    unsigned int type = *(unsigned int *)(finfo + 20);
    unsigned int smem_len = *(unsigned int *)(finfo + 24);
    unsigned int line_length = *(unsigned int *)(finfo + 48);
    printf("  FSCREENINFO: type=%u smem_len=%u line_length=%u id=%.16s\n",
           type, smem_len, line_length, (char *)finfo);
    if (type != 0) { // FB_TYPE_PACKED_PIXELS
        printf("FAIL: type != FB_TYPE_PACKED_PIXELS (type=%u)\n", type);
        close(fd);
        return 1;
    }

    // Step 5: FBIOGET_VSCREENINFO
    unsigned char vinfo[160];
    memset(vinfo, 0, sizeof(vinfo));
    if (ioctl(fd, FBIOGET_VSCREENINFO, vinfo) < 0) {
        printf("FAIL: FBIOGET_VSCREENINFO: %s (errno=%d)\n", strerror(errno), errno);
        close(fd);
        return 1;
    }
    unsigned int *v = (unsigned int *)vinfo;
    printf("  VSCREENINFO: %ux%u %ubpp\n", v[0], v[1], v[6]);

    // Step 6: mmap (what Xorg does for drawing)
    if (smem_len > 0) {
        void *fb = mmap(NULL, smem_len, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
        if (fb == MAP_FAILED) {
            printf("FAIL: mmap: %s (errno=%d)\n", strerror(errno), errno);
        } else {
            printf("  mmap: OK at %p (%u bytes)\n", fb, smem_len);
            // Write a red pixel to top-left
            ((unsigned int *)fb)[0] = 0xFFFF0000;
            munmap(fb, smem_len);
        }
    }

    close(fd);
    printf("PASS: all fb0 probe checks passed\n");
    return 0;
}
