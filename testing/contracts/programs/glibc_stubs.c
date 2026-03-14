/* Contract: glibc init syscall stubs return expected values without crashing. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <unistd.h>
#include <sys/syscall.h>
#include <sched.h>

int main(void) {
    /* rseq: on Kevlar returns ENOSYS, on Linux returns EINVAL (args invalid)
       or EPERM (already registered). Either error is acceptable. */
    long ret = syscall(SYS_rseq, 0, 0, 0, 0);
    if (ret != -1) {
        printf("CONTRACT_FAIL glibc_stubs_rseq: expected error, got %ld\n", ret);
        return 1;
    }
    printf("glibc_stubs_rseq: ok\n");

    /* sched_setaffinity should succeed (no-op) */
    unsigned long mask = 1;
    ret = syscall(SYS_sched_setaffinity, 0, sizeof(mask), &mask);
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

    /* sched_setscheduler should succeed (no-op) */
    struct sched_param param = { .sched_priority = 0 };
    ret = syscall(SYS_sched_setscheduler, 0, SCHED_OTHER, &param);
    if (ret != 0) {
        printf("CONTRACT_FAIL glibc_stubs_setscheduler: ret=%ld errno=%d\n", ret, errno);
        return 1;
    }
    printf("glibc_stubs_setscheduler: ok\n");

    /* clone3: Kevlar returns ENOSYS, Linux returns EFAULT/EINVAL for null args.
       Either error is acceptable — glibc falls back to clone(). */
#ifdef SYS_clone3
    ret = syscall(SYS_clone3, 0, 0);
    if (ret != -1) {
        printf("CONTRACT_FAIL glibc_stubs_clone3: expected error, got %ld\n", ret);
        return 1;
    }
    printf("glibc_stubs_clone3: ok\n");
#else
    printf("glibc_stubs_clone3: ok\n");
#endif

    printf("CONTRACT_PASS\n");
    return 0;
}
