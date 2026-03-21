/* Contract: settimeofday and clock_settime are accepted (return 0) or
 * return EPERM on non-root; either is valid since both are stubs. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>

int main(void) {
    /* settimeofday: set to current time (no real change) */
    struct timeval tv;
    gettimeofday(&tv, NULL);
    int ret = settimeofday(&tv, NULL);
    if (ret == 0) {
        printf("settimeofday: ok\n");
    } else if (errno == EPERM) {
        printf("settimeofday: ok\n");
    } else {
        printf("CONTRACT_FAIL settimeofday: ret=%d errno=%d\n", ret, errno);
        return 1;
    }

    /* clock_settime: set CLOCK_REALTIME to current time */
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    ret = clock_settime(CLOCK_REALTIME, &ts);
    if (ret == 0) {
        printf("clock_settime: ok\n");
    } else if (errno == EPERM) {
        printf("clock_settime: ok\n");
    } else {
        printf("CONTRACT_FAIL clock_settime: ret=%d errno=%d\n", ret, errno);
        return 1;
    }

    printf("CONTRACT_PASS\n");
    return 0;
}
