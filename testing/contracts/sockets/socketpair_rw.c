/* Contract: socketpair bidirectional I/O; close one → other reads EOF. */
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

int main(void) {
    int fds[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, fds) != 0) {
        printf("CONTRACT_FAIL socketpair: errno=%d\n", errno);
        return 1;
    }

    /* Write on [0] → readable on [1] */
    write(fds[0], "ping", 4);
    char buf[16] = {0};
    int n = read(fds[1], buf, sizeof(buf));
    if (n != 4 || memcmp(buf, "ping", 4) != 0) {
        printf("CONTRACT_FAIL forward: n=%d buf=%s\n", n, buf);
        return 1;
    }
    printf("forward: ok\n");

    /* Write on [1] → readable on [0] */
    write(fds[1], "pong", 4);
    memset(buf, 0, sizeof(buf));
    n = read(fds[0], buf, sizeof(buf));
    if (n != 4 || memcmp(buf, "pong", 4) != 0) {
        printf("CONTRACT_FAIL reverse: n=%d buf=%s\n", n, buf);
        return 1;
    }
    printf("reverse: ok\n");

    /* Close [1] → read on [0] returns 0 (EOF) */
    close(fds[1]);
    n = read(fds[0], buf, sizeof(buf));
    if (n != 0) {
        printf("CONTRACT_FAIL eof: n=%d\n", n);
        return 1;
    }
    printf("eof: ok\n");

    close(fds[0]);
    printf("CONTRACT_PASS\n");
    return 0;
}
