/* Contract: nanosleep sleeps approximately correct duration;
 * invalid tv_nsec → EINVAL. */
#include <errno.h>
#include <stdio.h>
#include <time.h>

int main(void) {
    struct timespec before, after;

    /* 50ms sleep → at least 40ms elapsed */
    clock_gettime(CLOCK_MONOTONIC, &before);
    struct timespec req = {.tv_sec = 0, .tv_nsec = 50000000}; /* 50ms */
    int ret = nanosleep(&req, NULL);
    clock_gettime(CLOCK_MONOTONIC, &after);

    if (ret != 0) {
        printf("CONTRACT_FAIL nanosleep: ret=%d errno=%d\n", ret, errno);
        return 1;
    }

    long elapsed_ns = (after.tv_sec - before.tv_sec) * 1000000000L +
                      (after.tv_nsec - before.tv_nsec);
    long elapsed_ms = elapsed_ns / 1000000;
    if (elapsed_ms < 40) {
        printf("CONTRACT_FAIL duration: elapsed=%ldms expected>=40ms\n", elapsed_ms);
        return 1;
    }
    printf("nanosleep: ok\n");

    /* Invalid tv_nsec → EINVAL */
    struct timespec bad = {.tv_sec = 0, .tv_nsec = 1000000000}; /* 10^9 is invalid */
    errno = 0;
    ret = nanosleep(&bad, NULL);
    if (ret != -1 || errno != EINVAL) {
        printf("CONTRACT_FAIL einval: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("einval: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
