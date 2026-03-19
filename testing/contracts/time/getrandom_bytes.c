/* Contract: getrandom returns entropy; not all zeros;
 * GRND_NONBLOCK accepted. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <sys/random.h>
#include <unistd.h>

int main(void) {
    unsigned char buf[32] = {0};

    ssize_t n = getrandom(buf, sizeof(buf), 0);
    if (n != (ssize_t)sizeof(buf)) {
        printf("CONTRACT_FAIL getrandom: n=%ld errno=%d\n", (long)n, errno);
        return 1;
    }
    printf("getrandom: ok n=%ld\n", (long)n);

    /* Not all zeros (probability 2^-256) */
    int all_zero = 1;
    for (int i = 0; i < 32; i++) {
        if (buf[i] != 0) {
            all_zero = 0;
            break;
        }
    }
    if (all_zero) {
        printf("CONTRACT_FAIL entropy: all zeros\n");
        return 1;
    }
    printf("entropy: ok\n");

    /* GRND_NONBLOCK accepted */
    unsigned char buf2[16];
    n = getrandom(buf2, sizeof(buf2), GRND_NONBLOCK);
    if (n < 0 && errno != EAGAIN) {
        printf("CONTRACT_FAIL grnd_nonblock: n=%ld errno=%d\n", (long)n, errno);
        return 1;
    }
    printf("grnd_nonblock: ok n=%ld\n", (long)n);

    printf("CONTRACT_PASS\n");
    return 0;
}
