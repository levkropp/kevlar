/* Contract: flock LOCK_EX/LOCK_SH/LOCK_UN accepted on regular fds;
 * LOCK_NB does not block; EBADF on bad fd. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <sys/file.h>
#include <unistd.h>

int main(void) {
    int fd = open("/dev/null", O_RDWR);
    if (fd < 0) {
        printf("CONTRACT_FAIL open: errno=%d\n", errno);
        return 1;
    }

    /* LOCK_EX should succeed (or at least not error fatally) */
    int ret = flock(fd, LOCK_EX);
    if (ret != 0) {
        printf("CONTRACT_FAIL lock_ex: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("lock_ex: ok\n");

    /* LOCK_UN */
    ret = flock(fd, LOCK_UN);
    if (ret != 0) {
        printf("CONTRACT_FAIL lock_un: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("lock_un: ok\n");

    /* LOCK_SH */
    ret = flock(fd, LOCK_SH);
    if (ret != 0) {
        printf("CONTRACT_FAIL lock_sh: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("lock_sh: ok\n");

    /* LOCK_SH | LOCK_NB (non-blocking) */
    ret = flock(fd, LOCK_SH | LOCK_NB);
    if (ret != 0) {
        printf("CONTRACT_FAIL lock_sh_nb: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("lock_sh_nb: ok\n");

    flock(fd, LOCK_UN);

    /* EBADF on closed fd */
    close(fd);
    errno = 0;
    ret = flock(fd, LOCK_EX);
    if (ret != -1 || errno != EBADF) {
        printf("CONTRACT_FAIL ebadf: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("ebadf: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
