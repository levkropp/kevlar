/* Contract: close_range closes targeted fds; no-op on empty range;
 * survivors still usable. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <unistd.h>
#include <sys/syscall.h>

static int sys_close_range(unsigned int first, unsigned int last, unsigned int flags) {
    return syscall(SYS_close_range, first, last, flags);
}

int main(void) {
    /* Open several fds */
    int fd1 = open("/dev/null", O_RDONLY);
    int fd2 = open("/dev/null", O_RDONLY);
    int fd3 = open("/dev/null", O_RDONLY);
    if (fd1 < 0 || fd2 < 0 || fd3 < 0) {
        printf("CONTRACT_FAIL open: fd1=%d fd2=%d fd3=%d\n", fd1, fd2, fd3);
        return 1;
    }
    printf("opened: fd1=%d fd2=%d fd3=%d\n", fd1, fd2, fd3);

    /* Close just fd2 */
    int ret = sys_close_range(fd2, fd2, 0);
    if (ret != 0) {
        printf("CONTRACT_FAIL close_range: ret=%d errno=%d\n", ret, errno);
        return 1;
    }

    /* fd2 should be closed */
    errno = 0;
    int r = fcntl(fd2, F_GETFD);
    if (r != -1 || errno != EBADF) {
        printf("CONTRACT_FAIL fd2_closed: r=%d errno=%d\n", r, errno);
        return 1;
    }
    printf("fd2_closed: ok\n");

    /* fd1 and fd3 should still be open */
    if (fcntl(fd1, F_GETFD) < 0) {
        printf("CONTRACT_FAIL fd1_alive: errno=%d\n", errno);
        return 1;
    }
    if (fcntl(fd3, F_GETFD) < 0) {
        printf("CONTRACT_FAIL fd3_alive: errno=%d\n", errno);
        return 1;
    }
    printf("survivors: ok\n");

    /* No-op on empty/high range */
    ret = sys_close_range(9000, 9999, 0);
    if (ret != 0) {
        printf("CONTRACT_FAIL empty_range: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("empty_range: ok\n");

    close(fd1);
    close(fd3);
    printf("CONTRACT_PASS\n");
    return 0;
}
