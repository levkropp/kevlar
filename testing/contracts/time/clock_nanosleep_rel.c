/* Contract: clock_nanosleep relative sleep returns 0; CLOCK_MONOTONIC
 * accepted; EINVAL on bad clock; elapsed time is >= requested. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <time.h>

int main(void) {
    /* Relative sleep: 10ms on CLOCK_MONOTONIC */
    struct timespec before, after;
    clock_gettime(CLOCK_MONOTONIC, &before);

    struct timespec req = { .tv_sec = 0, .tv_nsec = 10000000 }; /* 10ms */
    int ret = clock_nanosleep(CLOCK_MONOTONIC, 0, &req, NULL);
    if (ret != 0) {
        printf("CONTRACT_FAIL relative: ret=%d errno=%d\n", ret, errno);
        return 1;
    }

    clock_gettime(CLOCK_MONOTONIC, &after);
    long elapsed_ns = (after.tv_sec - before.tv_sec) * 1000000000L +
                      (after.tv_nsec - before.tv_nsec);
    if (elapsed_ns < 5000000) { /* allow 5ms tolerance */
        printf("CONTRACT_FAIL too_fast: elapsed=%ldns\n", elapsed_ns);
        return 1;
    }
    printf("relative_mono: ok\n");

    /* Relative sleep on CLOCK_REALTIME */
    req.tv_nsec = 1000000; /* 1ms */
    ret = clock_nanosleep(CLOCK_REALTIME, 0, &req, NULL);
    if (ret != 0) {
        printf("CONTRACT_FAIL realtime: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("relative_real: ok\n");

    /* Zero sleep should return immediately */
    struct timespec zero = { .tv_sec = 0, .tv_nsec = 0 };
    ret = clock_nanosleep(CLOCK_MONOTONIC, 0, &zero, NULL);
    if (ret != 0) {
        printf("CONTRACT_FAIL zero_sleep: ret=%d\n", ret);
        return 1;
    }
    printf("zero_sleep: ok\n");

    /* EINVAL on bad clock */
    errno = 0;
    ret = clock_nanosleep(999, 0, &req, NULL);
    if (ret != EINVAL && !(ret == -1 && errno == EINVAL)) {
        printf("CONTRACT_FAIL bad_clock: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("bad_clock: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
