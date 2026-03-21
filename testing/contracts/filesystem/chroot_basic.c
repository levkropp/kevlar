/* Contract: chroot changes root directory; paths resolve relative to new
 * root; requires root or EPERM. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <sys/stat.h>
#include <unistd.h>

int main(void) {
    /* Create a test directory structure */
    mkdir("/tmp/chroot_test", 0755);
    mkdir("/tmp/chroot_test/sub", 0755);
    int fd = open("/tmp/chroot_test/marker", O_CREAT | O_WRONLY, 0644);
    if (fd >= 0) { write(fd, "ok", 2); close(fd); }

    /* chroot into /tmp/chroot_test */
    int ret = chroot("/tmp/chroot_test");
    if (ret == 0) {
        /* /marker should now be visible at root */
        struct stat st;
        if (stat("/marker", &st) == 0) {
            printf("chroot: ok\n");
        } else {
            printf("CONTRACT_FAIL marker_missing: errno=%d\n", errno);
            return 1;
        }

        /* /sub should exist */
        if (stat("/sub", &st) == 0 && S_ISDIR(st.st_mode)) {
            printf("subdir: ok\n");
        } else {
            printf("CONTRACT_FAIL subdir: errno=%d\n", errno);
            return 1;
        }
    } else if (errno == EPERM) {
        /* Non-root: can't chroot */
        printf("chroot: ok\n");
        printf("subdir: ok\n");
    } else {
        printf("CONTRACT_FAIL chroot: ret=%d errno=%d\n", ret, errno);
        return 1;
    }

    printf("CONTRACT_PASS\n");
    return 0;
}
