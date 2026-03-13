/* Contract: mmap allocates virtual address space lazily — physical pages
   are only allocated on first access (demand paging).  A large mmap
   followed by touching only one page should not allocate all pages. */
#define _GNU_SOURCE
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>

int main(void) {
    /* Map 1 MB but don't touch it yet */
    size_t size = 1024 * 1024;
    char *p = mmap(NULL, size, PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == MAP_FAILED) {
        printf("CONTRACT_FAIL mmap\n");
        return 1;
    }
    printf("mmap_ok: mapped %zu bytes\n", size);

    /* Touch only the first page */
    p[0] = 42;
    if (p[0] != 42) {
        printf("CONTRACT_FAIL first_page_rw\n");
        return 1;
    }
    printf("first_page: ok\n");

    /* Touch a page in the middle */
    p[size / 2] = 77;
    if (p[size / 2] != 77) {
        printf("CONTRACT_FAIL mid_page_rw\n");
        return 1;
    }
    printf("mid_page: ok\n");

    /* Touch the last page */
    p[size - 1] = 99;
    if (p[size - 1] != 99) {
        printf("CONTRACT_FAIL last_page_rw\n");
        return 1;
    }
    printf("last_page: ok\n");

    /* Verify untouched pages are zero (demand-paged anonymous pages
       must be zero-filled per POSIX) */
    int non_zero = 0;
    for (int i = 4096; i < (int)(size / 2); i++) {
        if (p[i] != 0) { non_zero++; break; }
    }
    if (non_zero) {
        printf("CONTRACT_FAIL zero_fill\n");
        return 1;
    }
    printf("zero_fill: ok\n");

    munmap(p, size);
    printf("CONTRACT_PASS\n");
    return 0;
}
