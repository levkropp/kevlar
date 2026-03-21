/* Contract: sched_getscheduler returns SCHED_OTHER (0) by default;
 * sched_setscheduler accepts SCHED_OTHER without error. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <unistd.h>

#define SCHED_OTHER 0

/* Use raw syscalls because musl may not wrap these on all arches */
static int my_sched_getscheduler(int pid) {
    return syscall(SYS_sched_getscheduler, pid);
}

static int my_sched_setscheduler(int pid, int policy, const void *param) {
    return syscall(SYS_sched_setscheduler, pid, policy, param);
}

int main(void) {
    /* sched_getscheduler for current process */
    int policy = my_sched_getscheduler(0);
    if (policy < 0) {
        printf("CONTRACT_FAIL getscheduler: ret=%d errno=%d\n", policy, errno);
        return 1;
    }
    if (policy != SCHED_OTHER) {
        printf("CONTRACT_FAIL default_policy: got=%d expected=%d\n",
               policy, SCHED_OTHER);
        return 1;
    }
    printf("getscheduler: ok policy=%d\n", policy);

    /* sched_setscheduler to SCHED_OTHER */
    struct { int sched_priority; } param = { 0 };
    int ret = my_sched_setscheduler(0, SCHED_OTHER, &param);
    if (ret != 0) {
        printf("CONTRACT_FAIL setscheduler: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("setscheduler: ok\n");

    /* Verify still SCHED_OTHER */
    policy = my_sched_getscheduler(0);
    if (policy != SCHED_OTHER) {
        printf("CONTRACT_FAIL after_set: got=%d\n", policy);
        return 1;
    }
    printf("verify: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
