/* Contract: mremap returns ENOSYS (not implemented). */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <sys/mman.h>
#include <unistd.h>

int main(void) {
    size_t pgsz = sysconf(_SC_PAGESIZE);

    void *addr = mmap(NULL, pgsz, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (addr == MAP_FAILED) {
        printf("CONTRACT_FAIL mmap: errno=%d\n", errno);
        return 1;
    }

    errno = 0;
    void *addr2 = mremap(addr, pgsz, pgsz * 2, MREMAP_MAYMOVE);
    if (addr2 != MAP_FAILED) {
        printf("mremap_succeeded: ok\n");
        munmap(addr2, pgsz * 2);
        printf("CONTRACT_PASS\n");
        return 0;
    }
    if (errno == ENOSYS) {
        printf("mremap_enosys: ok\n");
        munmap(addr, pgsz);
        printf("CONTRACT_PASS\n");
        return 0;
    }

    printf("CONTRACT_FAIL mremap: errno=%d expected ENOSYS(%d)\n", errno, ENOSYS);
    munmap(addr, pgsz);
    return 1;
}
