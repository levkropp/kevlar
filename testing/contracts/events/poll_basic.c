/* Contract: poll() readiness on pipe and /dev/null;
 * timeout=0 returns immediately. */
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdio.h>
#include <unistd.h>

int main(void) {
    int fds[2];
    pipe(fds);

    /* Write end is always POLLOUT */
    struct pollfd pfd = {.fd = fds[1], .events = POLLOUT};
    int ret = poll(&pfd, 1, 0);
    if (ret != 1 || !(pfd.revents & POLLOUT)) {
        printf("CONTRACT_FAIL write_pollout: ret=%d revents=0x%x\n", ret, pfd.revents);
        return 1;
    }
    printf("write_pollout: ok\n");

    /* Read end is not POLLIN until data written */
    pfd.fd = fds[0];
    pfd.events = POLLIN;
    ret = poll(&pfd, 1, 0);
    if (ret != 0) {
        printf("CONTRACT_FAIL empty_pipe: ret=%d\n", ret);
        return 1;
    }
    printf("empty_pipe: ok\n");

    /* Write data → read end becomes POLLIN */
    write(fds[1], "x", 1);
    ret = poll(&pfd, 1, 0);
    if (ret != 1 || !(pfd.revents & POLLIN)) {
        printf("CONTRACT_FAIL pollin: ret=%d revents=0x%x\n", ret, pfd.revents);
        return 1;
    }
    printf("pollin: ok\n");
    char buf;
    read(fds[0], &buf, 1); /* drain */

    /* /dev/null: POLLOUT always */
    int devnull = open("/dev/null", O_WRONLY);
    pfd.fd = devnull;
    pfd.events = POLLOUT;
    ret = poll(&pfd, 1, 0);
    if (ret != 1 || !(pfd.revents & POLLOUT)) {
        printf("CONTRACT_FAIL devnull_pollout: ret=%d revents=0x%x\n", ret, pfd.revents);
        return 1;
    }
    printf("devnull_pollout: ok\n");
    close(devnull);

    /* timeout=0 with nothing ready returns 0 immediately */
    pfd.fd = fds[0];
    pfd.events = POLLIN;
    ret = poll(&pfd, 1, 0);
    if (ret != 0) {
        printf("CONTRACT_FAIL timeout_zero: ret=%d\n", ret);
        return 1;
    }
    printf("timeout_zero: ok\n");

    close(fds[0]);
    close(fds[1]);
    printf("CONTRACT_PASS\n");
    return 0;
}
