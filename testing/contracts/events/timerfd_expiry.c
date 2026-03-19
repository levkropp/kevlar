/* Contract: timerfd_create + timerfd_settime fires after interval;
 * poll reports readable; read returns expiry count. */
#include <errno.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <sys/timerfd.h>
#include <unistd.h>

int main(void) {
    int tfd = timerfd_create(CLOCK_MONOTONIC, TFD_NONBLOCK);
    if (tfd < 0) {
        printf("CONTRACT_FAIL timerfd_create: errno=%d\n", errno);
        return 1;
    }

    /* Read before armed → EAGAIN */
    uint64_t val;
    errno = 0;
    int n = read(tfd, &val, sizeof(val));
    if (n != -1 || errno != EAGAIN) {
        printf("CONTRACT_FAIL unarmed: n=%d errno=%d\n", n, errno);
        return 1;
    }
    printf("unarmed_eagain: ok\n");

    /* Arm 50ms one-shot */
    struct itimerspec its = {
        .it_value = {.tv_sec = 0, .tv_nsec = 50000000},
        .it_interval = {.tv_sec = 0, .tv_nsec = 0}
    };
    if (timerfd_settime(tfd, 0, &its, NULL) != 0) {
        printf("CONTRACT_FAIL settime: errno=%d\n", errno);
        return 1;
    }

    /* Poll for readability (200ms timeout, generous) */
    struct pollfd pfd = {.fd = tfd, .events = POLLIN};
    int ret = poll(&pfd, 1, 200);
    if (ret != 1 || !(pfd.revents & POLLIN)) {
        printf("CONTRACT_FAIL poll: ret=%d revents=0x%x\n", ret, pfd.revents);
        return 1;
    }
    printf("poll_ready: ok\n");

    /* Read expiry count */
    n = read(tfd, &val, sizeof(val));
    if (n != 8 || val < 1) {
        printf("CONTRACT_FAIL read_expiry: n=%d val=%llu\n", n, (unsigned long long)val);
        return 1;
    }
    printf("expiry_count: %llu\n", (unsigned long long)val);

    /* After read, non-blocking read → EAGAIN (one-shot expired) */
    errno = 0;
    n = read(tfd, &val, sizeof(val));
    if (n != -1 || errno != EAGAIN) {
        printf("CONTRACT_FAIL post_read: n=%d errno=%d\n", n, errno);
        return 1;
    }
    printf("post_read_eagain: ok\n");

    close(tfd);
    printf("CONTRACT_PASS\n");
    return 0;
}
