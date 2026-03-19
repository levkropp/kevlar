/* Contract: fcntl F_GETFD/F_SETFD, F_GETFL/F_SETFL, F_DUPFD. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <unistd.h>

int main(void) {
    int fd = open("/dev/null", O_RDWR);
    if (fd < 0) {
        printf("CONTRACT_FAIL open: errno=%d\n", errno);
        return 1;
    }

    /* F_GETFD: no CLOEXEC by default */
    int fdflags = fcntl(fd, F_GETFD);
    if (fdflags & FD_CLOEXEC) {
        printf("CONTRACT_FAIL default_nocloexec: flags=0x%x\n", fdflags);
        return 1;
    }
    printf("default_nocloexec: ok\n");

    /* F_SETFD: set CLOEXEC */
    fcntl(fd, F_SETFD, FD_CLOEXEC);
    fdflags = fcntl(fd, F_GETFD);
    if (!(fdflags & FD_CLOEXEC)) {
        printf("CONTRACT_FAIL set_cloexec: flags=0x%x\n", fdflags);
        return 1;
    }
    printf("set_cloexec: ok\n");

    /* F_GETFL: check access mode */
    int flflags = fcntl(fd, F_GETFL);
    if ((flflags & O_ACCMODE) != O_RDWR) {
        printf("CONTRACT_FAIL getfl_rdwr: flags=0x%x\n", flflags);
        return 1;
    }
    printf("getfl_rdwr: ok\n");

    /* F_SETFL: add O_NONBLOCK */
    fcntl(fd, F_SETFL, flflags | O_NONBLOCK);
    int newfl = fcntl(fd, F_GETFL);
    if (!(newfl & O_NONBLOCK)) {
        printf("CONTRACT_FAIL set_nonblock: flags=0x%x\n", newfl);
        return 1;
    }
    printf("set_nonblock: ok\n");

    /* F_DUPFD: returns fd >= arg */
    int fd2 = fcntl(fd, F_DUPFD, 100);
    if (fd2 < 100) {
        printf("CONTRACT_FAIL dupfd_min: fd2=%d\n", fd2);
        return 1;
    }
    printf("dupfd_min: ok fd2=%d\n", fd2);

    /* F_DUPFD_CLOEXEC */
    int fd3 = fcntl(fd, F_DUPFD_CLOEXEC, 200);
    if (fd3 < 200) {
        printf("CONTRACT_FAIL dupfd_cloexec_min: fd3=%d\n", fd3);
        return 1;
    }
    fdflags = fcntl(fd3, F_GETFD);
    if (!(fdflags & FD_CLOEXEC)) {
        printf("CONTRACT_FAIL dupfd_cloexec_flag: flags=0x%x\n", fdflags);
        return 1;
    }
    printf("dupfd_cloexec: ok fd3=%d\n", fd3);

    close(fd3);
    close(fd2);
    close(fd);
    printf("CONTRACT_PASS\n");
    return 0;
}
