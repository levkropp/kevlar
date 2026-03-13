/* Contract: sched_yield() exists and returns 0.
   Also validates sched_getaffinity returns a valid CPU mask. */
#define _GNU_SOURCE
#include <stdio.h>
#include <sched.h>
#include <errno.h>

int main(void) {
    /* sched_yield should succeed */
    if (sched_yield() != 0) {
        printf("CONTRACT_FAIL sched_yield errno=%d\n", errno);
        return 1;
    }
    printf("sched_yield: ok\n");

    /* sched_getaffinity should return a valid mask */
    cpu_set_t cpus;
    CPU_ZERO(&cpus);
    if (sched_getaffinity(0, sizeof(cpus), &cpus) != 0) {
        printf("CONTRACT_FAIL sched_getaffinity errno=%d\n", errno);
        return 1;
    }
    int count = CPU_COUNT(&cpus);
    if (count < 1) {
        printf("CONTRACT_FAIL cpu_count=%d\n", count);
        return 1;
    }
    printf("sched_getaffinity: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
