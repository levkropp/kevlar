/* Contract: sched_setaffinity accepts valid mask; sched_getaffinity
 * returns non-empty mask; both return 0 on success. */
#define _GNU_SOURCE
#include <errno.h>
#include <sched.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int main(void) {
    cpu_set_t mask;

    /* sched_getaffinity: should return at least 1 CPU */
    CPU_ZERO(&mask);
    int ret = sched_getaffinity(0, sizeof(mask), &mask);
    if (ret != 0) {
        printf("CONTRACT_FAIL getaffinity: ret=%d errno=%d\n", ret, errno);
        return 1;
    }

    int count = CPU_COUNT(&mask);
    if (count < 1) {
        printf("CONTRACT_FAIL no_cpus: count=%d\n", count);
        return 1;
    }
    printf("getaffinity: ok\n");

    /* sched_setaffinity: set to current mask (roundtrip) */
    ret = sched_setaffinity(0, sizeof(mask), &mask);
    if (ret != 0) {
        printf("CONTRACT_FAIL setaffinity: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("setaffinity: ok\n");

    /* Verify getaffinity still returns CPUs after set */
    cpu_set_t mask2;
    CPU_ZERO(&mask2);
    ret = sched_getaffinity(0, sizeof(mask2), &mask2);
    if (ret != 0) {
        printf("CONTRACT_FAIL getaffinity2: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    int count2 = CPU_COUNT(&mask2);
    if (count2 < 1) {
        printf("CONTRACT_FAIL roundtrip: count=%d\n", count2);
        return 1;
    }
    printf("roundtrip: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
