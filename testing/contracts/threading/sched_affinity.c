/* Contract: sched_getaffinity returns at least CPU 0;
 * sched_setaffinity + get roundtrip consistent. */
#define _GNU_SOURCE
#include <errno.h>
#include <sched.h>
#include <stdio.h>
#include <unistd.h>

int main(void) {
    cpu_set_t set;
    CPU_ZERO(&set);

    if (sched_getaffinity(0, sizeof(set), &set) != 0) {
        printf("CONTRACT_FAIL getaffinity: errno=%d\n", errno);
        return 1;
    }

    /* At least CPU 0 should be set */
    if (!CPU_ISSET(0, &set)) {
        printf("CONTRACT_FAIL cpu0: not set\n");
        return 1;
    }

    int count = CPU_COUNT(&set);
    printf("cpu_count: ok\n");
    if (count < 1) {
        printf("CONTRACT_FAIL count: %d\n", count);
        return 1;
    }
    printf("getaffinity: ok\n");

    /* Set affinity to just CPU 0, then read back */
    cpu_set_t single;
    CPU_ZERO(&single);
    CPU_SET(0, &single);
    if (sched_setaffinity(0, sizeof(single), &single) != 0) {
        printf("CONTRACT_FAIL setaffinity: errno=%d\n", errno);
        return 1;
    }

    cpu_set_t readback;
    CPU_ZERO(&readback);
    sched_getaffinity(0, sizeof(readback), &readback);
    if (!CPU_ISSET(0, &readback)) {
        printf("CONTRACT_FAIL roundtrip: cpu0 not set\n");
        return 1;
    }
    printf("roundtrip: ok\n");

    /* Restore original mask */
    sched_setaffinity(0, sizeof(set), &set);

    printf("CONTRACT_PASS\n");
    return 0;
}
