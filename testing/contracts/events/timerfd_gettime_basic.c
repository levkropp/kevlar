/* Contract: timerfd_gettime returns zero for unarmed timer; after
 * timerfd_settime with interval, gettime returns non-zero remaining;
 * disarming (settime 0,0) resets gettime to zero. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/timerfd.h>
#include <unistd.h>

int main(void) {
    int fd = timerfd_create(CLOCK_MONOTONIC, 0);
    if (fd < 0) {
        printf("CONTRACT_FAIL create: errno=%d\n", errno);
        return 1;
    }
    printf("create: ok\n");

    /* Unarmed timer: gettime should return zeros */
    struct itimerspec cur;
    memset(&cur, 0xff, sizeof(cur));
    int ret = timerfd_gettime(fd, &cur);
    if (ret != 0) {
        printf("CONTRACT_FAIL gettime_unarmed: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    if (cur.it_value.tv_sec != 0 || cur.it_value.tv_nsec != 0 ||
        cur.it_interval.tv_sec != 0 || cur.it_interval.tv_nsec != 0) {
        printf("CONTRACT_FAIL unarmed_zero: val=%ld.%09ld int=%ld.%09ld\n",
               (long)cur.it_value.tv_sec, cur.it_value.tv_nsec,
               (long)cur.it_interval.tv_sec, cur.it_interval.tv_nsec);
        return 1;
    }
    printf("gettime_unarmed: ok\n");

    /* Arm timer: 10 second one-shot (won't fire during test) */
    struct itimerspec its;
    memset(&its, 0, sizeof(its));
    its.it_value.tv_sec = 10;
    its.it_value.tv_nsec = 0;
    ret = timerfd_settime(fd, 0, &its, NULL);
    if (ret != 0) {
        printf("CONTRACT_FAIL settime: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("settime: ok\n");

    /* gettime should return non-zero remaining */
    memset(&cur, 0, sizeof(cur));
    ret = timerfd_gettime(fd, &cur);
    if (ret != 0) {
        printf("CONTRACT_FAIL gettime_armed: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    if (cur.it_value.tv_sec == 0 && cur.it_value.tv_nsec == 0) {
        printf("CONTRACT_FAIL armed_nonzero: remaining is zero\n");
        return 1;
    }
    printf("gettime_armed: ok\n");

    /* Disarm: set value to 0,0 */
    memset(&its, 0, sizeof(its));
    ret = timerfd_settime(fd, 0, &its, NULL);
    if (ret != 0) {
        printf("CONTRACT_FAIL disarm: ret=%d errno=%d\n", ret, errno);
        return 1;
    }

    /* gettime should return zeros again */
    ret = timerfd_gettime(fd, &cur);
    if (ret != 0) {
        printf("CONTRACT_FAIL gettime_disarmed: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    if (cur.it_value.tv_sec != 0 || cur.it_value.tv_nsec != 0) {
        printf("CONTRACT_FAIL disarmed_zero: val=%ld.%09ld\n",
               (long)cur.it_value.tv_sec, cur.it_value.tv_nsec);
        return 1;
    }
    printf("gettime_disarmed: ok\n");

    close(fd);
    printf("CONTRACT_PASS\n");
    return 0;
}
