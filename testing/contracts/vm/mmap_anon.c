/* Contract: anonymous mmap returns zeroed pages; munmap releases them;
 * MAP_FIXED replaces existing mapping. */
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

int main(void) {
    /* Pages must be zero-initialised */
    char *p = mmap(NULL, 8192, PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == MAP_FAILED) {
        printf("CONTRACT_FAIL mmap_anon\n");
        return 1;
    }
    for (int i = 0; i < 8192; i++) {
        if (p[i] != 0) {
            printf("CONTRACT_FAIL zero_init: byte %d = %d\n", i, (unsigned char)p[i]);
            return 1;
        }
    }
    printf("zero_init: ok\n");

    /* Write pattern then re-map with MAP_FIXED | MAP_ANONYMOUS: must be fresh zeros */
    memset(p, 0xAA, 4096);
    char *q = mmap(p, 4096, PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
    if (q != p) {
        printf("CONTRACT_FAIL map_fixed: got %p expected %p\n", (void *)q, (void *)p);
        return 1;
    }
    if (p[0] != 0) {
        printf("CONTRACT_FAIL map_fixed_zero: byte 0 = %d\n", (unsigned char)p[0]);
        return 1;
    }
    printf("map_fixed_zero: ok\n");

    munmap(p, 8192);
    printf("CONTRACT_PASS\n");
    return 0;
}
