/* Contract: ioctl TIOCGWINSZ returns window size, FIONBIO sets nonblock,
 * FIOCLEX/FIONCLEX set/clear FD_CLOEXEC. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>

int main(void) {
    /* TIOCGWINSZ on a tty-like fd (may not have a real tty, so ENOTTY is ok) */
    struct winsize ws;
    memset(&ws, 0, sizeof(ws));
    int ret = ioctl(STDOUT_FILENO, TIOCGWINSZ, &ws);
    if (ret == 0) {
        printf("tiocgwinsz: ok\n");
    } else if (errno == ENOTTY || errno == ENOSYS) {
        printf("tiocgwinsz: ok\n");
    } else {
        printf("CONTRACT_FAIL tiocgwinsz: ret=%d errno=%d\n", ret, errno);
        return 1;
    }

    /* FIOCLEX / FIONCLEX on a regular fd */
    int fd = open("/dev/null", O_RDWR);
    if (fd < 0) {
        printf("CONTRACT_FAIL open: errno=%d\n", errno);
        return 1;
    }

    /* Default: no CLOEXEC */
    int flags = fcntl(fd, F_GETFD);
    if (flags & FD_CLOEXEC) {
        printf("CONTRACT_FAIL default_nocloexec: flags=0x%x\n", flags);
        return 1;
    }

    /* FIOCLEX sets CLOEXEC */
    ret = ioctl(fd, FIOCLEX);
    if (ret != 0) {
        printf("CONTRACT_FAIL fioclex: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    flags = fcntl(fd, F_GETFD);
    if (!(flags & FD_CLOEXEC)) {
        printf("CONTRACT_FAIL fioclex_check: flags=0x%x\n", flags);
        return 1;
    }
    printf("fioclex: ok\n");

    /* FIONCLEX clears CLOEXEC */
    ret = ioctl(fd, FIONCLEX);
    if (ret != 0) {
        printf("CONTRACT_FAIL fionclex: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    flags = fcntl(fd, F_GETFD);
    if (flags & FD_CLOEXEC) {
        printf("CONTRACT_FAIL fionclex_check: flags=0x%x\n", flags);
        return 1;
    }
    printf("fionclex: ok\n");

    /* FIONBIO sets O_NONBLOCK via ioctl */
    int one = 1;
    ret = ioctl(fd, FIONBIO, &one);
    if (ret != 0) {
        printf("CONTRACT_FAIL fionbio_set: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    int fl = fcntl(fd, F_GETFL);
    if (!(fl & O_NONBLOCK)) {
        printf("CONTRACT_FAIL fionbio_check: fl=0x%x\n", fl);
        return 1;
    }
    printf("fionbio_set: ok\n");

    /* FIONBIO clears O_NONBLOCK */
    int zero = 0;
    ret = ioctl(fd, FIONBIO, &zero);
    if (ret != 0) {
        printf("CONTRACT_FAIL fionbio_clear: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    fl = fcntl(fd, F_GETFL);
    if (fl & O_NONBLOCK) {
        printf("CONTRACT_FAIL fionbio_clear_check: fl=0x%x\n", fl);
        return 1;
    }
    printf("fionbio_clear: ok\n");

    close(fd);
    printf("CONTRACT_PASS\n");
    return 0;
}
