/* Contract: CLOCK_REALTIME returns plausible wall-clock;
 * tv_nsec in valid range; REALTIME_COARSE works. */
#include <errno.h>
#include <stdio.h>
#include <time.h>

int main(void) {
    struct timespec ts;

    if (clock_gettime(CLOCK_REALTIME, &ts) != 0) {
        printf("CONTRACT_FAIL clock_gettime: errno=%d\n", errno);
        return 1;
    }

    /* Plausible: after 2023-11-14 */
    if (ts.tv_sec < 1700000000L) {
        printf("CONTRACT_FAIL plausible: tv_sec=%ld\n", ts.tv_sec);
        return 1;
    }
    printf("realtime: ok tv_sec=%ld\n", ts.tv_sec);

    /* tv_nsec in [0, 999999999] */
    if (ts.tv_nsec < 0 || ts.tv_nsec > 999999999L) {
        printf("CONTRACT_FAIL tv_nsec: %ld\n", ts.tv_nsec);
        return 1;
    }
    printf("tv_nsec_range: ok\n");

    /* CLOCK_REALTIME_COARSE works */
    errno = 0;
    if (clock_gettime(CLOCK_REALTIME_COARSE, &ts) != 0) {
        printf("CONTRACT_FAIL realtime_coarse: errno=%d\n", errno);
        return 1;
    }
    if (ts.tv_sec < 1700000000L) {
        printf("CONTRACT_FAIL coarse_plausible: tv_sec=%ld\n", ts.tv_sec);
        return 1;
    }
    printf("realtime_coarse: ok tv_sec=%ld\n", ts.tv_sec);

    printf("CONTRACT_PASS\n");
    return 0;
}
