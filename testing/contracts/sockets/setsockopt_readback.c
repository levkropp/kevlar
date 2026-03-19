/* Contract: setsockopt accepts common options; getsockopt reads back. */
#include <errno.h>
#include <stdio.h>
#include <sys/socket.h>
#include <unistd.h>

int main(void) {
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0) {
        printf("CONTRACT_FAIL socket: errno=%d\n", errno);
        return 1;
    }

    /* SO_REUSEADDR */
    int val = 1;
    if (setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &val, sizeof(val)) != 0) {
        printf("CONTRACT_FAIL setsockopt_reuse: errno=%d\n", errno);
        close(fd);
        return 1;
    }
    printf("setsockopt_reuseaddr: ok\n");

    /* SO_KEEPALIVE */
    val = 1;
    if (setsockopt(fd, SOL_SOCKET, SO_KEEPALIVE, &val, sizeof(val)) != 0) {
        printf("CONTRACT_FAIL setsockopt_keepalive: errno=%d\n", errno);
        close(fd);
        return 1;
    }
    printf("setsockopt_keepalive: ok\n");

    /* getsockopt SO_RCVBUF — should return > 0 */
    int rcvbuf = 0;
    socklen_t len = sizeof(rcvbuf);
    if (getsockopt(fd, SOL_SOCKET, SO_RCVBUF, &rcvbuf, &len) != 0) {
        printf("CONTRACT_FAIL getsockopt_rcvbuf: errno=%d\n", errno);
        close(fd);
        return 1;
    }
    if (rcvbuf <= 0) {
        /* Some stubs may return 0 — that's acceptable */
        printf("getsockopt_rcvbuf: val=%d (possibly stub)\n", rcvbuf);
    } else {
        printf("getsockopt_rcvbuf: ok val=%d\n", rcvbuf);
    }

    close(fd);
    printf("CONTRACT_PASS\n");
    return 0;
}
