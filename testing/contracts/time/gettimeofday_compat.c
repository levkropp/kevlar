/* Contract: gettimeofday agrees with clock_gettime within 1 second. */
#include <errno.h>
#include <stdio.h>
#include <sys/time.h>
#include <time.h>

int main(void) {
    struct timeval tv;
    struct timespec ts;

    if (gettimeofday(&tv, NULL) != 0) {
        printf("CONTRACT_FAIL gettimeofday: errno=%d\n", errno);
        return 1;
    }
    if (clock_gettime(CLOCK_REALTIME, &ts) != 0) {
        printf("CONTRACT_FAIL clock_gettime: errno=%d\n", errno);
        return 1;
    }

    long diff = ts.tv_sec - tv.tv_sec;
    if (diff < -1 || diff > 1) {
        printf("CONTRACT_FAIL agree: gtod=%ld clock=%ld diff=%ld\n",
               tv.tv_sec, ts.tv_sec, diff);
        return 1;
    }
    printf("agree: ok diff=%ld\n", diff);

    /* tv_usec in valid range */
    if (tv.tv_usec < 0 || tv.tv_usec >= 1000000) {
        printf("CONTRACT_FAIL tv_usec: %ld\n", tv.tv_usec);
        return 1;
    }
    printf("tv_usec_range: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
