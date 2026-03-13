/* Contract: brk() grows the heap; new pages are zero-initialised. */
#define _GNU_SOURCE
#include <stdio.h>
#include <unistd.h>
#include <sys/syscall.h>

/* Use raw brk syscall directly: musl 1.2.x deprecated sbrk() for non-zero
   arguments (stub always returns -ENOMEM).  The brk syscall returns the
   current program break on success, or the unchanged break on failure. */
static unsigned long raw_brk(unsigned long addr)
{
    return (unsigned long)syscall(SYS_brk, addr);
}

int main(void) {
    /* brk(0) returns current program break */
    unsigned long start = raw_brk(0);
    if (start == 0) {
        printf("CONTRACT_FAIL brk_query\n");
        return 1;
    }

    /* Grow by two pages */
    unsigned long new_end = start + 8192;
    unsigned long result = raw_brk(new_end);
    if (result != new_end) {
        printf("CONTRACT_FAIL brk_grow (asked %lx, got %lx)\n", new_end, result);
        return 1;
    }

    /* New region must be zeroed */
    char *p = (char *)start;
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
