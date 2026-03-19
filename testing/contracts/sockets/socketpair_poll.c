/* Contract: poll readiness on socketpair;
 * fresh pair POLLOUT; after write POLLIN; after close POLLHUP. */
#include <errno.h>
#include <poll.h>
#include <stdio.h>
#include <sys/socket.h>
#include <unistd.h>

int main(void) {
    int fds[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, fds) != 0) {
        printf("CONTRACT_FAIL socketpair: errno=%d\n", errno);
        return 1;
    }

    /* Fresh: [0] should be writable (POLLOUT) */
    struct pollfd pfd = {.fd = fds[0], .events = POLLIN | POLLOUT};
    int ret = poll(&pfd, 1, 0);
    if (ret < 1 || !(pfd.revents & POLLOUT)) {
        printf("CONTRACT_FAIL fresh_pollout: ret=%d revents=0x%x\n", ret, pfd.revents);
        return 1;
    }
    /* Should NOT have POLLIN (no data) */
    if (pfd.revents & POLLIN) {
        printf("CONTRACT_FAIL fresh_no_pollin: revents=0x%x\n", pfd.revents);
        return 1;
    }
    printf("fresh_pollout: ok\n");

    /* Write on [1] → [0] becomes POLLIN */
    write(fds[1], "data", 4);
    pfd.fd = fds[0];
    pfd.events = POLLIN;
    ret = poll(&pfd, 1, 0);
    if (ret != 1 || !(pfd.revents & POLLIN)) {
        printf("CONTRACT_FAIL data_pollin: ret=%d revents=0x%x\n", ret, pfd.revents);
        return 1;
    }
    printf("data_pollin: ok\n");
    char buf[8];
    read(fds[0], buf, 4); /* drain */

    /* Close [1] → [0] gets POLLHUP */
    close(fds[1]);
    pfd.fd = fds[0];
    pfd.events = POLLIN;
    ret = poll(&pfd, 1, 0);
    if (ret < 1 || !(pfd.revents & POLLIN)) {
        printf("CONTRACT_FAIL close_pollin: ret=%d revents=0x%x\n", ret, pfd.revents);
        return 1;
    }
    if (!(pfd.revents & POLLHUP)) {
        printf("CONTRACT_FAIL close_pollhup: revents=0x%x missing POLLHUP\n", pfd.revents);
        return 1;
    }
    printf("close_pollhup: ok\n");

    close(fds[0]);
    printf("CONTRACT_PASS\n");
    return 0;
}
