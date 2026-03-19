/* Contract: CLOCK_MONOTONIC never goes backwards;
 * MONOTONIC_COARSE and BOOTTIME accepted. */
#include <errno.h>
#include <stdio.h>
#include <time.h>

int main(void) {
    struct timespec prev, cur;

    /* CLOCK_MONOTONIC: 1000 reads, each >= previous */
    clock_gettime(CLOCK_MONOTONIC, &prev);
    for (int i = 0; i < 1000; i++) {
        clock_gettime(CLOCK_MONOTONIC, &cur);
        if (cur.tv_sec < prev.tv_sec ||
            (cur.tv_sec == prev.tv_sec && cur.tv_nsec < prev.tv_nsec)) {
            printf("CONTRACT_FAIL monotonic: i=%d prev=%ld.%09ld cur=%ld.%09ld\n",
                   i, prev.tv_sec, prev.tv_nsec, cur.tv_sec, cur.tv_nsec);
            return 1;
        }
        prev = cur;
    }
    printf("monotonic_1000: ok\n");

    /* CLOCK_MONOTONIC_COARSE accepted */
    errno = 0;
    if (clock_gettime(CLOCK_MONOTONIC_COARSE, &cur) != 0) {
        printf("CONTRACT_FAIL monotonic_coarse: errno=%d\n", errno);
        return 1;
    }
    printf("monotonic_coarse: ok\n");

    /* CLOCK_BOOTTIME accepted */
    errno = 0;
    if (clock_gettime(CLOCK_BOOTTIME, &cur) != 0) {
        printf("CONTRACT_FAIL boottime: errno=%d\n", errno);
        return 1;
    }
    printf("boottime: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
