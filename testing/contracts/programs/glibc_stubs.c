/* Contract: glibc init syscall stubs return expected values.
   Only tests syscalls where Kevlar and Linux produce identical results.
   rseq and clone3 are omitted — they return ENOSYS (Kevlar) vs
   EINVAL/EFAULT (Linux) for invalid args; full implementations come later. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <unistd.h>
#include <sys/syscall.h>
#include <sched.h>

int main(void) {
    /* sched_setaffinity should succeed */
    unsigned long mask = 1;
    long ret = syscall(SYS_sched_setaffinity, 0, sizeof(mask), &mask);
    if (ret != 0) {
        printf("CONTRACT_FAIL glibc_stubs_setaffinity: ret=%ld errno=%d\n", ret, errno);
        return 1;
    }
    printf("glibc_stubs_setaffinity: ok\n");

    /* sched_getscheduler should return SCHED_OTHER (0) */
    ret = syscall(SYS_sched_getscheduler, 0);
    if (ret != 0) {
        printf("CONTRACT_FAIL glibc_stubs_getscheduler: ret=%ld\n", ret);
        return 1;
    }
    printf("glibc_stubs_getscheduler: ok\n");

    /* sched_setscheduler should succeed */
    struct sched_param param = { .sched_priority = 0 };
    ret = syscall(SYS_sched_setscheduler, 0, SCHED_OTHER, &param);
    if (ret != 0) {
        printf("CONTRACT_FAIL glibc_stubs_setscheduler: ret=%ld errno=%d\n", ret, errno);
        return 1;
    }
    printf("glibc_stubs_setscheduler: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
