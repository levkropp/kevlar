/* Contract: nice() affects scheduling priority.
   setpriority/getpriority correctly store and retrieve nice values.
   Note: on Linux, unprivileged processes can increase nice (lower prio)
   but cannot decrease it.  This test only increases nice to be portable. */
#define _GNU_SOURCE
#include <stdio.h>
#include <sys/resource.h>
#include <errno.h>

int main(void) {
    /* Default nice should be 0 */
    errno = 0;
    int prio = getpriority(PRIO_PROCESS, 0);
    if (errno != 0) {
        printf("CONTRACT_FAIL getpriority errno=%d\n", errno);
        return 1;
    }
    printf("initial_nice: %d\n", prio);
    if (prio != 0) {
        printf("CONTRACT_FAIL initial_nice expected=0 got=%d\n", prio);
        return 1;
    }

    /* Set nice to 5 (increase = lower priority, always allowed) */
    if (setpriority(PRIO_PROCESS, 0, 5) != 0) {
        printf("CONTRACT_FAIL setpriority_5 errno=%d\n", errno);
        return 1;
    }
    errno = 0;
    prio = getpriority(PRIO_PROCESS, 0);
    if (errno != 0 || prio != 5) {
        printf("CONTRACT_FAIL nice5 expected=5 got=%d errno=%d\n", prio, errno);
        return 1;
    }
    printf("nice_5: ok\n");

    /* Set nice to 10 */
    if (setpriority(PRIO_PROCESS, 0, 10) != 0) {
        printf("CONTRACT_FAIL setpriority_10 errno=%d\n", errno);
        return 1;
    }
    errno = 0;
    prio = getpriority(PRIO_PROCESS, 0);
    if (errno != 0 || prio != 10) {
        printf("CONTRACT_FAIL nice10 expected=10 got=%d errno=%d\n", prio, errno);
        return 1;
    }
    printf("nice_10: ok\n");

    /* Set nice to 19 (maximum) */
    if (setpriority(PRIO_PROCESS, 0, 19) != 0) {
        printf("CONTRACT_FAIL setpriority_19 errno=%d\n", errno);
        return 1;
    }
    errno = 0;
    prio = getpriority(PRIO_PROCESS, 0);
    if (errno != 0 || prio != 19) {
        printf("CONTRACT_FAIL nice19 expected=19 got=%d errno=%d\n", prio, errno);
        return 1;
    }
    printf("nice_19: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
