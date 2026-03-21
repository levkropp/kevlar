/* Contract: fallocate on regular fd returns 0 (accepted); EBADF on
 * closed fd; file size may or may not change (tmpfs ignores prealloc). */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <sys/stat.h>
#include <unistd.h>

/* fallocate mode flags */
#ifndef FALLOC_FL_KEEP_SIZE
#define FALLOC_FL_KEEP_SIZE 0x01
#endif

int main(void) {
    int fd = open("/tmp/falloc_test", O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (fd < 0) {
        printf("CONTRACT_FAIL open: errno=%d\n", errno);
        return 1;
    }

    /* Basic fallocate: preallocate 4096 bytes */
    int ret = fallocate(fd, 0, 0, 4096);
    if (ret != 0) {
        printf("CONTRACT_FAIL fallocate: ret=%d errno=%d\n", ret, errno);
        close(fd);
        return 1;
    }
    printf("fallocate_basic: ok\n");

    /* fallocate with KEEP_SIZE */
    ret = fallocate(fd, FALLOC_FL_KEEP_SIZE, 0, 8192);
    if (ret != 0) {
        printf("CONTRACT_FAIL keep_size: ret=%d errno=%d\n", ret, errno);
        close(fd);
        return 1;
    }
    printf("fallocate_keepsize: ok\n");

    /* Verify file is accessible after fallocate */
    struct stat st;
    if (fstat(fd, &st) != 0) {
        printf("CONTRACT_FAIL fstat: errno=%d\n", errno);
        close(fd);
        return 1;
    }
    /* On Linux, fallocate(0) extends the file; on tmpfs stubs it may not */
    printf("fstat_after: ok\n");

    close(fd);
    unlink("/tmp/falloc_test");

    /* EBADF on closed fd */
    errno = 0;
    ret = fallocate(fd, 0, 0, 4096);
    if (ret != -1 || errno != EBADF) {
        printf("CONTRACT_FAIL ebadf: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("ebadf: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
