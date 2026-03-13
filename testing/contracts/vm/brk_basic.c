/* Contract: brk()/sbrk() grows the heap; new pages are zero-initialised. */
#include <stdio.h>
#include <unistd.h>

int main(void) {
    /* sbrk(0) returns current program break */
    void *start = sbrk(0);
    if (start == (void *)-1) {
        printf("CONTRACT_FAIL sbrk(0)\n");
        return 1;
    }

    /* Grow by two pages */
    void *ret = sbrk(8192);
    if (ret == (void *)-1) {
        printf("CONTRACT_FAIL sbrk_grow\n");
        return 1;
    }

    /* New region must be zeroed */
    char *p = (char *)ret;
    for (int i = 0; i < 8192; i++) {
        if (p[i] != 0) {
            printf("CONTRACT_FAIL brk_zero: byte %d = %d\n", i, (unsigned char)p[i]);
            return 1;
        }
    }
    printf("brk_zero: ok\n");

    /* Write to it and read back */
    p[0] = 99;
    p[4095] = 77;
    if (p[0] != 99 || p[4095] != 77) {
        printf("CONTRACT_FAIL brk_rw\n");
        return 1;
    }
    printf("brk_rw: ok\n");
    printf("CONTRACT_PASS\n");
    return 0;
}
