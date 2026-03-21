/* Contract: posix_fadvise returns 0 (advisory hint accepted); various
 * advice values accepted; EBADF on closed fd. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <unistd.h>

#ifndef POSIX_FADV_NORMAL
#define POSIX_FADV_NORMAL    0
#define POSIX_FADV_RANDOM    1
#define POSIX_FADV_SEQUENTIAL 2
#define POSIX_FADV_WILLNEED  3
#define POSIX_FADV_DONTNEED  4
#define POSIX_FADV_NOREUSE   5
#endif

int main(void) {
    int fd = open("/dev/null", O_RDONLY);
    if (fd < 0) {
        printf("CONTRACT_FAIL open: errno=%d\n", errno);
        return 1;
    }

    /* posix_fadvise returns error code directly (not via errno) */
    int ret = posix_fadvise(fd, 0, 0, POSIX_FADV_NORMAL);
    if (ret != 0) {
        printf("CONTRACT_FAIL normal: ret=%d\n", ret);
        close(fd);
        return 1;
    }
    printf("fadv_normal: ok\n");

    ret = posix_fadvise(fd, 0, 4096, POSIX_FADV_SEQUENTIAL);
    if (ret != 0) {
        printf("CONTRACT_FAIL sequential: ret=%d\n", ret);
        close(fd);
        return 1;
    }
    printf("fadv_sequential: ok\n");

    ret = posix_fadvise(fd, 0, 0, POSIX_FADV_DONTNEED);
    if (ret != 0) {
        printf("CONTRACT_FAIL dontneed: ret=%d\n", ret);
        close(fd);
        return 1;
    }
    printf("fadv_dontneed: ok\n");

    close(fd);

    /* EBADF on closed fd */
    ret = posix_fadvise(fd, 0, 0, POSIX_FADV_NORMAL);
    if (ret != EBADF) {
        printf("CONTRACT_FAIL ebadf: ret=%d\n", ret);
        return 1;
    }
    printf("ebadf: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
