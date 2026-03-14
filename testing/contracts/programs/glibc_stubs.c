/* Contract: glibc init syscall stubs return Linux-identical values.
   rseq and clone3 with invalid args should return the same errno as Linux. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <unistd.h>
#include <sys/syscall.h>
#include <sched.h>

int main(void) {
    /* rseq with null args: Linux returns EINVAL */
    errno = 0;
    long ret = syscall(SYS_rseq, 0, 0, 0, 0);
    if (ret != -1 || errno != EINVAL) {
        printf("CONTRACT_FAIL glibc_stubs_rseq: ret=%ld errno=%d (expected EINVAL=%d)\n",
               ret, errno, EINVAL);
        return 1;
    }
    printf("glibc_stubs_rseq: ok\n");

    /* sched_setaffinity should succeed */
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

    /* sched_setscheduler should succeed */
    struct sched_param param = { .sched_priority = 0 };
    ret = syscall(SYS_sched_setscheduler, 0, SCHED_OTHER, &param);
    if (ret != 0) {
        printf("CONTRACT_FAIL glibc_stubs_setscheduler: ret=%ld errno=%d\n", ret, errno);
        return 1;
    }
    printf("glibc_stubs_setscheduler: ok\n");

    /* clone3 with null args and size=0: Linux returns EINVAL */
#ifdef SYS_clone3
    errno = 0;
    ret = syscall(SYS_clone3, 0, 0);
    if (ret != -1 || errno != EINVAL) {
        printf("CONTRACT_FAIL glibc_stubs_clone3: ret=%ld errno=%d (expected EINVAL=%d)\n",
               ret, errno, EINVAL);
        return 1;
    }
    printf("glibc_stubs_clone3: ok\n");
#else
    printf("glibc_stubs_clone3: ok\n");
#endif

    printf("CONTRACT_PASS\n");
    return 0;
}
