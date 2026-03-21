/* Contract: ppoll with timeout returns 0 on expiry; returns ready count
 * when fd is readable; works with NULL sigmask. */
#define _GNU_SOURCE
#include <errno.h>
#include <poll.h>
#include <stdio.h>
#include <unistd.h>

int main(void) {
    int pfd[2];
    if (pipe(pfd) != 0) {
        printf("CONTRACT_FAIL pipe: errno=%d\n", errno);
        return 1;
    }

    /* ppoll with short timeout on empty pipe — should return 0 (timeout) */
    struct pollfd fds[1];
    fds[0].fd = pfd[0];
    fds[0].events = POLLIN;
    fds[0].revents = 0;

    struct timespec ts = { .tv_sec = 0, .tv_nsec = 10000000 }; /* 10ms */
    int ret = ppoll(fds, 1, &ts, NULL);
    if (ret != 0) {
        printf("CONTRACT_FAIL timeout: ret=%d errno=%d revents=0x%x\n",
               ret, errno, fds[0].revents);
        return 1;
    }
    printf("timeout: ok\n");

    /* Write to pipe, then ppoll — should return 1 (readable) */
    char c = 'x';
    write(pfd[1], &c, 1);

    fds[0].revents = 0;
    ts.tv_sec = 1;
    ts.tv_nsec = 0;
    ret = ppoll(fds, 1, &ts, NULL);
    if (ret != 1) {
        printf("CONTRACT_FAIL readable: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    if (!(fds[0].revents & POLLIN)) {
        printf("CONTRACT_FAIL revents: 0x%x\n", fds[0].revents);
        return 1;
    }
    printf("readable: ok revents=0x%x\n", fds[0].revents);

    /* Drain the byte */
    read(pfd[0], &c, 1);

    /* ppoll with zero timeout = nonblocking poll */
    fds[0].revents = 0;
    ts.tv_sec = 0;
    ts.tv_nsec = 0;
    ret = ppoll(fds, 1, &ts, NULL);
    if (ret != 0) {
        printf("CONTRACT_FAIL zero_timeout: ret=%d\n", ret);
        return 1;
    }
    printf("zero_timeout: ok\n");

    /* Close write end — read end should get POLLHUP */
    close(pfd[1]);
    fds[0].revents = 0;
    ts.tv_sec = 0;
    ts.tv_nsec = 10000000;
    ret = ppoll(fds, 1, &ts, NULL);
    if (ret >= 1 && (fds[0].revents & POLLHUP)) {
        printf("pollhup: ok revents=0x%x\n", fds[0].revents);
    } else if (ret == 0) {
        /* Some implementations don't wake on HUP immediately */
        printf("pollhup: ok (timeout, acceptable)\n");
    } else {
        printf("CONTRACT_FAIL pollhup: ret=%d revents=0x%x\n", ret, fds[0].revents);
        return 1;
    }

    close(pfd[0]);
    printf("CONTRACT_PASS\n");
    return 0;
}
