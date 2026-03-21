/* Contract: clock_getres returns valid resolution for CLOCK_MONOTONIC
 * and CLOCK_REALTIME; EINVAL for invalid clock; NULL res pointer accepted. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <time.h>

int main(void) {
    struct timespec res;

    /* CLOCK_MONOTONIC */
    res.tv_sec = -1;
    res.tv_nsec = -1;
    int ret = clock_getres(CLOCK_MONOTONIC, &res);
    if (ret != 0) {
        printf("CONTRACT_FAIL mono: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    if (res.tv_sec < 0 || res.tv_nsec < 0 || (res.tv_sec == 0 && res.tv_nsec == 0)) {
        printf("CONTRACT_FAIL mono_val: sec=%ld nsec=%ld\n",
               (long)res.tv_sec, (long)res.tv_nsec);
        return 1;
    }
    printf("clock_monotonic: ok sec=%ld nsec=%ld\n",
           (long)res.tv_sec, (long)res.tv_nsec);

    /* CLOCK_REALTIME */
    res.tv_sec = -1;
    res.tv_nsec = -1;
    ret = clock_getres(CLOCK_REALTIME, &res);
    if (ret != 0) {
        printf("CONTRACT_FAIL real: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    if (res.tv_sec < 0 || res.tv_nsec < 0 || (res.tv_sec == 0 && res.tv_nsec == 0)) {
        printf("CONTRACT_FAIL real_val: sec=%ld nsec=%ld\n",
               (long)res.tv_sec, (long)res.tv_nsec);
        return 1;
    }
    printf("clock_realtime: ok sec=%ld nsec=%ld\n",
           (long)res.tv_sec, (long)res.tv_nsec);

    /* NULL res pointer — just validates clock, writes nothing */
    ret = clock_getres(CLOCK_MONOTONIC, NULL);
    if (ret != 0) {
        printf("CONTRACT_FAIL null_res: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("null_res: ok\n");

    /* Invalid clock ID */
    errno = 0;
    ret = clock_getres(999, &res);
    if (ret != -1 || errno != EINVAL) {
        printf("CONTRACT_FAIL bad_clock: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("bad_clock: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
