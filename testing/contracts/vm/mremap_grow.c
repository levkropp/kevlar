/* Contract: mremap grows an anonymous mapping with MREMAP_MAYMOVE. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

int main(void) {
    size_t pgsz = sysconf(_SC_PAGESIZE);

    /* Allocate 1 page. */
    void *addr = mmap(NULL, pgsz, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (addr == MAP_FAILED) {
        printf("CONTRACT_FAIL mmap: errno=%d\n", errno);
        return 1;
    }

    /* Write a sentinel to verify data is preserved after remap. */
    memset(addr, 0xAB, pgsz);
    printf("mmap_ok: 1 page\n");

    /* Grow to 2 pages with MREMAP_MAYMOVE. */
    void *addr2 = mremap(addr, pgsz, pgsz * 2, MREMAP_MAYMOVE);
    if (addr2 == MAP_FAILED) {
        printf("CONTRACT_FAIL mremap_grow: errno=%d\n", errno);
        munmap(addr, pgsz);
        return 1;
    }
    printf("mremap_grow: ok\n");

    /* Verify old data survived the remap. */
    unsigned char *p = (unsigned char *)addr2;
    int ok = 1;
    for (size_t i = 0; i < pgsz; i++) {
        if (p[i] != 0xAB) {
            ok = 0;
            break;
        }
    }
    if (!ok) {
        printf("CONTRACT_FAIL data_preserved: sentinel corrupted\n");
        munmap(addr2, pgsz * 2);
        return 1;
    }
    printf("data_preserved: ok\n");

    /* New pages should be zero-filled. */
    ok = 1;
    for (size_t i = pgsz; i < pgsz * 2; i++) {
        if (p[i] != 0) {
            ok = 0;
            break;
        }
    }
    if (!ok) {
        printf("CONTRACT_FAIL zero_fill: new pages not zero\n");
        munmap(addr2, pgsz * 2);
        return 1;
    }
    printf("zero_fill: ok\n");

    /* Shrink back to 1 page. */
    void *addr3 = mremap(addr2, pgsz * 2, pgsz, 0);
    if (addr3 == MAP_FAILED) {
        printf("CONTRACT_FAIL mremap_shrink: errno=%d\n", errno);
        munmap(addr2, pgsz * 2);
        return 1;
    }
    printf("mremap_shrink: ok\n");

    /* Verify data still intact after shrink. */
    p = (unsigned char *)addr3;
    ok = 1;
    for (size_t i = 0; i < pgsz; i++) {
        if (p[i] != 0xAB) {
            ok = 0;
            break;
        }
    }
    if (!ok) {
        printf("CONTRACT_FAIL shrink_data: sentinel corrupted\n");
        munmap(addr3, pgsz);
        return 1;
    }
    printf("shrink_data: ok\n");

    munmap(addr3, pgsz);
    printf("CONTRACT_PASS\n");
    return 0;
}
