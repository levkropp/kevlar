/* Contract: SOCK_DGRAM socketpair sendto/recvfrom preserves messages. */
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

int main(void) {
    int fds[2];
    if (socketpair(AF_UNIX, SOCK_DGRAM, 0, fds) != 0) {
        printf("CONTRACT_FAIL socketpair: errno=%d\n", errno);
        return 1;
    }

    /* sendto on [0], recvfrom on [1] */
    const char *msg = "datagram";
    ssize_t sent = sendto(fds[0], msg, strlen(msg), 0, NULL, 0);
    if (sent != (ssize_t)strlen(msg)) {
        printf("CONTRACT_FAIL sendto: sent=%ld errno=%d\n", (long)sent, errno);
        return 1;
    }
    printf("sendto: ok sent=%ld\n", (long)sent);

    char buf[32] = {0};
    ssize_t rcvd = recvfrom(fds[1], buf, sizeof(buf), 0, NULL, NULL);
    if (rcvd != (ssize_t)strlen(msg) || memcmp(buf, msg, strlen(msg)) != 0) {
        printf("CONTRACT_FAIL recvfrom: rcvd=%ld buf=%s\n", (long)rcvd, buf);
        return 1;
    }
    printf("recvfrom: ok\n");

    /* Send two messages — DGRAM preserves boundaries */
    sendto(fds[0], "AA", 2, 0, NULL, 0);
    sendto(fds[0], "BBB", 3, 0, NULL, 0);

    memset(buf, 0, sizeof(buf));
    rcvd = recvfrom(fds[1], buf, sizeof(buf), 0, NULL, NULL);
    if (rcvd != 2 || memcmp(buf, "AA", 2) != 0) {
        printf("CONTRACT_FAIL boundary1: rcvd=%ld buf=%s\n", (long)rcvd, buf);
        return 1;
    }
    memset(buf, 0, sizeof(buf));
    rcvd = recvfrom(fds[1], buf, sizeof(buf), 0, NULL, NULL);
    if (rcvd != 3 || memcmp(buf, "BBB", 3) != 0) {
        printf("CONTRACT_FAIL boundary2: rcvd=%ld buf=%s\n", (long)rcvd, buf);
        return 1;
    }
    printf("dgram_boundaries: ok\n");

    close(fds[0]);
    close(fds[1]);
    printf("CONTRACT_PASS\n");
    return 0;
}
