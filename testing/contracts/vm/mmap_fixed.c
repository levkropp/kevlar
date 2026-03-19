/* Contract: MAP_FIXED replaces existing mapping at exact address. */
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

int main(void) {
    size_t pgsz = sysconf(_SC_PAGESIZE);

    /* Allocate a page, write sentinel */
    void *addr = mmap(NULL, pgsz, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (addr == MAP_FAILED) {
        printf("CONTRACT_FAIL mmap1: errno=%d\n", errno);
        return 1;
    }
    *(unsigned char *)addr = 0xAA;
    printf("mmap1: ok addr=%p\n", addr);

    /* MAP_FIXED at same address — must return exact address */
    void *addr2 = mmap(addr, pgsz, PROT_READ | PROT_WRITE,
                        MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
    if (addr2 != addr) {
        printf("CONTRACT_FAIL mmap_fixed: addr2=%p expected=%p errno=%d\n",
               addr2, addr, errno);
        return 1;
    }
    printf("mmap_fixed: ok addr=%p\n", addr2);

    /* Sentinel must be gone (new zeroed page) */
    if (*(unsigned char *)addr2 != 0) {
        printf("CONTRACT_FAIL zero_check: got=0x%02x expected=0\n",
               *(unsigned char *)addr2);
        return 1;
    }
    printf("zero_check: ok\n");

    /* Write new data and verify */
    *(unsigned char *)addr2 = 0x42;
    if (*(unsigned char *)addr2 != 0x42) {
        printf("CONTRACT_FAIL write_check: got=0x%02x\n", *(unsigned char *)addr2);
        return 1;
    }
    printf("write_check: ok\n");

    munmap(addr2, pgsz);
    printf("CONTRACT_PASS\n");
    return 0;
}
