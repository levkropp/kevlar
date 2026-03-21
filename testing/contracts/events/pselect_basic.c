/* Contract: pselect6 works like select with timeout; returns 0 on
 * timeout; returns ready count when fd is readable. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <sys/select.h>
#include <unistd.h>

int main(void) {
    int pfd[2];
    if (pipe(pfd) != 0) {
        printf("CONTRACT_FAIL pipe: errno=%d\n", errno);
        return 1;
    }

    /* pselect with short timeout on empty pipe — should return 0 */
    fd_set rfds;
    FD_ZERO(&rfds);
    FD_SET(pfd[0], &rfds);

    struct timespec ts = { .tv_sec = 0, .tv_nsec = 10000000 }; /* 10ms */
    int ret = pselect(pfd[0] + 1, &rfds, NULL, NULL, &ts, NULL);
    if (ret != 0) {
        printf("CONTRACT_FAIL timeout: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("timeout: ok\n");

    /* Write data, then pselect should return 1 */
    write(pfd[1], "x", 1);

    FD_ZERO(&rfds);
    FD_SET(pfd[0], &rfds);
    ts.tv_sec = 1;
    ts.tv_nsec = 0;
    ret = pselect(pfd[0] + 1, &rfds, NULL, NULL, &ts, NULL);
    if (ret != 1) {
        printf("CONTRACT_FAIL readable: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    if (!FD_ISSET(pfd[0], &rfds)) {
        printf("CONTRACT_FAIL fd_not_set\n");
        return 1;
    }
    printf("readable: ok\n");

    /* Drain and close write end — pselect should see EOF */
    char c;
    read(pfd[0], &c, 1);
    close(pfd[1]);

    FD_ZERO(&rfds);
    FD_SET(pfd[0], &rfds);
    ts.tv_sec = 0;
    ts.tv_nsec = 10000000;
    ret = pselect(pfd[0] + 1, &rfds, NULL, NULL, &ts, NULL);
    if (ret >= 1 && FD_ISSET(pfd[0], &rfds)) {
        printf("eof_readable: ok\n");
    } else {
        printf("CONTRACT_FAIL eof: ret=%d\n", ret);
        close(pfd[0]);
        return 1;
    }

    close(pfd[0]);
    printf("CONTRACT_PASS\n");
    return 0;
}
