/* Contract: eventfd counter accumulation; read drains;
 * EFD_NONBLOCK read on zero → EAGAIN. */
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <sys/eventfd.h>
#include <unistd.h>

int main(void) {
    int efd = eventfd(0, EFD_NONBLOCK);
    if (efd < 0) {
        printf("CONTRACT_FAIL eventfd: errno=%d\n", errno);
        return 1;
    }

    /* Read on zero → EAGAIN */
    uint64_t val;
    errno = 0;
    int n = read(efd, &val, sizeof(val));
    if (n != -1 || errno != EAGAIN) {
        printf("CONTRACT_FAIL zero_eagain: n=%d errno=%d\n", n, errno);
        return 1;
    }
    printf("zero_eagain: ok\n");

    /* Write accumulates */
    uint64_t w1 = 3;
    uint64_t w2 = 5;
    write(efd, &w1, sizeof(w1));
    write(efd, &w2, sizeof(w2));

    n = read(efd, &val, sizeof(val));
    if (n != 8 || val != 8) {
        printf("CONTRACT_FAIL accumulate: n=%d val=%llu\n", n, (unsigned long long)val);
        return 1;
    }
    printf("accumulate: ok val=%llu\n", (unsigned long long)val);

    /* After drain, reading again → EAGAIN */
    errno = 0;
    n = read(efd, &val, sizeof(val));
    if (n != -1 || errno != EAGAIN) {
        printf("CONTRACT_FAIL post_drain: n=%d errno=%d\n", n, errno);
        return 1;
    }
    printf("post_drain_eagain: ok\n");

    /* Write size must be 8 bytes */
    uint32_t small = 1;
    errno = 0;
    n = write(efd, &small, sizeof(small));
    if (n != -1 || errno != EINVAL) {
        printf("CONTRACT_FAIL write_size: n=%d errno=%d\n", n, errno);
        return 1;
    }
    printf("write_size_check: ok\n");

    close(efd);
    printf("CONTRACT_PASS\n");
    return 0;
}
