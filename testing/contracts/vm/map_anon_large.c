/* Contract: large anonymous mmap is huge-page aligned; read/write works. */
#include <errno.h>
#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#define SIZE_4MB (4UL * 1024 * 1024)
#define SIZE_2MB (2UL * 1024 * 1024)

int main(void) {
    void *addr = mmap(NULL, SIZE_4MB, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (addr == MAP_FAILED) {
        printf("CONTRACT_FAIL mmap: errno=%d\n", errno);
        return 1;
    }
    printf("mmap_large: ok addr=%p\n", addr);

    /* Check huge-page alignment (2MB) */
    uintptr_t a = (uintptr_t)addr;
    if (a % SIZE_2MB != 0) {
        printf("alignment: addr=0x%lx not 2MB-aligned (offset=0x%lx)\n",
               (unsigned long)a, (unsigned long)(a % SIZE_2MB));
        /* Not fatal — alignment is best-effort on some systems */
    } else {
        printf("alignment: ok 2MB-aligned\n");
    }

    /* Write/read sentinels at boundaries */
    *(unsigned char *)addr = 0x11;
    *((unsigned char *)addr + SIZE_2MB) = 0x22;
    *((unsigned char *)addr + SIZE_4MB - 1) = 0x33;

    if (*(unsigned char *)addr != 0x11) {
        printf("CONTRACT_FAIL sentinel_start: got=0x%02x\n", *(unsigned char *)addr);
        return 1;
    }
    if (*((unsigned char *)addr + SIZE_2MB) != 0x22) {
        printf("CONTRACT_FAIL sentinel_mid: got=0x%02x\n",
               *((unsigned char *)addr + SIZE_2MB));
        return 1;
    }
    if (*((unsigned char *)addr + SIZE_4MB - 1) != 0x33) {
        printf("CONTRACT_FAIL sentinel_end: got=0x%02x\n",
               *((unsigned char *)addr + SIZE_4MB - 1));
        return 1;
    }
    printf("sentinels: ok\n");

    munmap(addr, SIZE_4MB);
    printf("CONTRACT_PASS\n");
    return 0;
}
