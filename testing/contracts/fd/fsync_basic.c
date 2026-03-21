/* Contract: fsync on regular fd returns 0; fsync after write returns 0;
 * EBADF on closed fd. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <unistd.h>

int main(void) {
    int fd = open("/tmp/fsync_test", O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (fd < 0) {
        printf("CONTRACT_FAIL open: errno=%d\n", errno);
        return 1;
    }

    /* fsync on empty file */
    int ret = fsync(fd);
    if (ret != 0) {
        printf("CONTRACT_FAIL fsync_empty: ret=%d errno=%d\n", ret, errno);
        close(fd);
        return 1;
    }
    printf("fsync_empty: ok\n");

    /* Write then fsync */
    write(fd, "data", 4);
    ret = fsync(fd);
    if (ret != 0) {
        printf("CONTRACT_FAIL fsync_data: ret=%d errno=%d\n", ret, errno);
        close(fd);
        return 1;
    }
    printf("fsync_data: ok\n");

    close(fd);
    unlink("/tmp/fsync_test");

    /* EBADF on closed fd */
    errno = 0;
    ret = fsync(fd);
    if (ret != -1 || errno != EBADF) {
        printf("CONTRACT_FAIL ebadf: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("ebadf: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
