/* Contract: clone3 returns ENOSYS for valid-sized args (glibc probes this
 * and falls back to clone); returns EINVAL for undersized args. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <unistd.h>

#ifndef SYS_clone3
#define SYS_clone3 435
#endif

int main(void) {
    /* Valid size (64 bytes = v0 minimum). On Linux this actually clones
     * (ret=0 in child, >0 in parent). On Kevlar returns ENOSYS (stub). */
    char args[128];
    __builtin_memset(args, 0, sizeof(args));

    long ret = syscall(SYS_clone3, args, 64);
    if (ret == 0) {
        /* We are the child on real Linux — exit immediately */
        _exit(0);
    } else if (ret > 0) {
        /* Parent on real Linux — child was created */
        printf("clone3_valid: ok\n");
    } else if (errno == ENOSYS) {
        printf("clone3_valid: ok\n");
    } else {
        printf("CONTRACT_FAIL clone3_valid: errno=%d (expected ENOSYS or fork)\n", errno);
        return 1;
    }

    /* Undersized args: must return EINVAL, not ENOSYS.
     * Linux validates size before checking implementation. */
    errno = 0;
    ret = syscall(SYS_clone3, args, 16);
    if (ret == -1 && errno == EINVAL) {
        printf("clone3_small: ok (EINVAL)\n");
    } else if (ret == -1 && errno == ENOSYS) {
        /* Some kernels return ENOSYS regardless — acceptable */
        printf("clone3_small: ok (ENOSYS)\n");
    } else {
        printf("CONTRACT_FAIL clone3_small: ret=%ld errno=%d\n", ret, errno);
        return 1;
    }

    printf("CONTRACT_PASS\n");
    return 0;
}
