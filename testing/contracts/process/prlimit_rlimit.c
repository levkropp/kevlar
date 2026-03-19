/* Contract: getrlimit returns sane values;
 * prlimit64 with NULL new == getrlimit. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <sys/resource.h>
#include <sys/syscall.h>
#include <unistd.h>

int main(void) {
    /* RLIMIT_STACK */
    struct rlimit rl;
    if (getrlimit(RLIMIT_STACK, &rl) != 0) {
        printf("CONTRACT_FAIL rlimit_stack: errno=%d\n", errno);
        return 1;
    }
    if (rl.rlim_cur == 0) {
        printf("CONTRACT_FAIL stack_cur: cur=%lu\n", (unsigned long)rl.rlim_cur);
        return 1;
    }
    printf("rlimit_stack: ok\n");

    /* RLIMIT_NOFILE */
    if (getrlimit(RLIMIT_NOFILE, &rl) != 0) {
        printf("CONTRACT_FAIL rlimit_nofile: errno=%d\n", errno);
        return 1;
    }
    if (rl.rlim_cur == 0) {
        printf("CONTRACT_FAIL nofile_cur: cur=%lu\n", (unsigned long)rl.rlim_cur);
        return 1;
    }
    printf("rlimit_nofile: ok\n");

    /* prlimit64 with NULL new_limit == getrlimit */
    struct rlimit rl2;
    int ret = syscall(SYS_prlimit64, 0, RLIMIT_NOFILE, NULL, &rl2);
    if (ret != 0) {
        printf("CONTRACT_FAIL prlimit64: errno=%d\n", errno);
        return 1;
    }
    if (rl2.rlim_cur != rl.rlim_cur || rl2.rlim_max != rl.rlim_max) {
        printf("CONTRACT_FAIL prlimit64_match: cur=%lu/%lu max=%lu/%lu\n",
               (unsigned long)rl2.rlim_cur, (unsigned long)rl.rlim_cur,
               (unsigned long)rl2.rlim_max, (unsigned long)rl.rlim_max);
        return 1;
    }
    printf("prlimit64: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
