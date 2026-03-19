/* Contract: madvise standard advice values accepted;
 * MADV_DONTNEED re-zeros anonymous pages. */
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>

int main(void) {
    char *p = mmap(NULL, 4096, PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == MAP_FAILED) {
        printf("CONTRACT_FAIL mmap\n");
        return 1;
    }

    /* MADV_NORMAL */
    if (madvise(p, 4096, MADV_NORMAL) != 0) {
        printf("CONTRACT_FAIL madv_normal\n");
        return 1;
    }
    printf("madv_normal: ok\n");

    /* MADV_SEQUENTIAL */
    if (madvise(p, 4096, MADV_SEQUENTIAL) != 0) {
        printf("CONTRACT_FAIL madv_sequential\n");
        return 1;
    }
    printf("madv_sequential: ok\n");

    /* MADV_RANDOM */
    if (madvise(p, 4096, MADV_RANDOM) != 0) {
        printf("CONTRACT_FAIL madv_random\n");
        return 1;
    }
    printf("madv_random: ok\n");

    /* MADV_WILLNEED */
    if (madvise(p, 4096, MADV_WILLNEED) != 0) {
        printf("CONTRACT_FAIL madv_willneed\n");
        return 1;
    }
    printf("madv_willneed: ok\n");

    /* MADV_DONTNEED: re-zeros anonymous pages */
    memset(p, 0xAA, 4096);
    if (madvise(p, 4096, MADV_DONTNEED) != 0) {
        printf("CONTRACT_FAIL madv_dontneed\n");
        return 1;
    }
    /* After DONTNEED, anonymous pages should be zeroed */
    if (p[0] != 0 || p[4095] != 0) {
        printf("CONTRACT_FAIL dontneed_zero: [0]=%d [4095]=%d\n",
               (unsigned char)p[0], (unsigned char)p[4095]);
        return 1;
    }
    printf("madv_dontneed_zero: ok\n");

    munmap(p, 4096);
    printf("CONTRACT_PASS\n");
    return 0;
}
