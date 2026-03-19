/* Contract: shutdown() half-close; SHUT_WR → peer reads EOF;
 * write after SHUT_WR → EPIPE; read still works. */
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

int main(void) {
    /* Ignore SIGPIPE so write returns EPIPE instead of killing us */
    signal(SIGPIPE, SIG_IGN);

    int fds[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, fds) != 0) {
        printf("CONTRACT_FAIL socketpair: errno=%d\n", errno);
        return 1;
    }

    /* SHUT_WR on fds[0]: peer reads EOF, but fds[0] can still read */
    shutdown(fds[0], SHUT_WR);

    /* Peer reads EOF */
    char buf[16];
    int n = read(fds[1], buf, sizeof(buf));
    if (n != 0) {
        printf("CONTRACT_FAIL shut_wr_eof: n=%d\n", n);
        return 1;
    }
    printf("shut_wr_eof: ok\n");

    /* Write on shut-down end → EPIPE */
    errno = 0;
    n = write(fds[0], "x", 1);
    if (n != -1 || errno != EPIPE) {
        printf("CONTRACT_FAIL shut_wr_epipe: n=%d errno=%d\n", n, errno);
        return 1;
    }
    printf("shut_wr_epipe: ok\n");

    /* fds[0] can still read from fds[1] */
    write(fds[1], "reply", 5);
    memset(buf, 0, sizeof(buf));
    n = read(fds[0], buf, sizeof(buf));
    if (n != 5 || memcmp(buf, "reply", 5) != 0) {
        printf("CONTRACT_FAIL read_after_shut_wr: n=%d buf=%s\n", n, buf);
        return 1;
    }
    printf("read_after_shut_wr: ok\n");

    close(fds[0]);
    close(fds[1]);
    printf("CONTRACT_PASS\n");
    return 0;
}
