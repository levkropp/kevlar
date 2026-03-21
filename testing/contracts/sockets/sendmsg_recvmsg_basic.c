/* Contract: sendmsg/recvmsg with iov scatter/gather works on unix
 * socketpair; multiple iovecs merged correctly on receive side. */
#define _GNU_SOURCE
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

    /* sendmsg with 2 iovecs */
    char part1[] = "hello ";
    char part2[] = "world";
    struct iovec iov_send[2] = {
        { .iov_base = part1, .iov_len = 6 },
        { .iov_base = part2, .iov_len = 5 },
    };
    struct msghdr msg_send;
    memset(&msg_send, 0, sizeof(msg_send));
    msg_send.msg_iov = iov_send;
    msg_send.msg_iovlen = 2;

    ssize_t sent = sendmsg(fds[0], &msg_send, 0);
    if (sent != 11) {
        printf("CONTRACT_FAIL sendmsg: sent=%ld errno=%d\n", (long)sent, errno);
        return 1;
    }
    printf("sendmsg: ok sent=%ld\n", (long)sent);

    /* recvmsg into single buffer */
    char buf[32] = {0};
    struct iovec iov_recv = { .iov_base = buf, .iov_len = sizeof(buf) };
    struct msghdr msg_recv;
    memset(&msg_recv, 0, sizeof(msg_recv));
    msg_recv.msg_iov = &iov_recv;
    msg_recv.msg_iovlen = 1;

    ssize_t rcvd = recvmsg(fds[1], &msg_recv, 0);
    if (rcvd != 11 || memcmp(buf, "hello world", 11) != 0) {
        printf("CONTRACT_FAIL recvmsg: rcvd=%ld buf=%s\n", (long)rcvd, buf);
        return 1;
    }
    printf("recvmsg: ok data=%.*s\n", (int)rcvd, buf);

    /* recvmsg into scatter (2 iovecs) */
    char r1[4] = {0}, r2[16] = {0};
    struct iovec iov_scatter[2] = {
        { .iov_base = r1, .iov_len = 4 },
        { .iov_base = r2, .iov_len = sizeof(r2) },
    };
    memset(&msg_recv, 0, sizeof(msg_recv));
    msg_recv.msg_iov = iov_scatter;
    msg_recv.msg_iovlen = 2;

    /* Send "ABCDEFGH" */
    char payload[] = "ABCDEFGH";
    ssize_t n = write(fds[0], payload, 8);
    if (n != 8) {
        printf("CONTRACT_FAIL write: n=%ld\n", (long)n);
        return 1;
    }

    rcvd = recvmsg(fds[1], &msg_recv, 0);
    if (rcvd != 8) {
        printf("CONTRACT_FAIL scatter_len: rcvd=%ld\n", (long)rcvd);
        return 1;
    }
    if (memcmp(r1, "ABCD", 4) != 0 || memcmp(r2, "EFGH", 4) != 0) {
        printf("CONTRACT_FAIL scatter_data: r1=%.*s r2=%.*s\n", 4, r1, 4, r2);
        return 1;
    }
    printf("scatter: ok r1=%.*s r2=%.*s\n", 4, r1, 4, r2);

    close(fds[0]);
    close(fds[1]);
    printf("CONTRACT_PASS\n");
    return 0;
}
