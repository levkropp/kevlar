/* Contract: hint flags MAP_STACK and MAP_GROWSDOWN must not cause mmap() to
 * fail — the kernel may ignore them but must not return EINVAL. */
#include <stdio.h>
#include <sys/mman.h>

#ifndef MAP_STACK
#define MAP_STACK 0x20000
#endif
#ifndef MAP_GROWSDOWN
#define MAP_GROWSDOWN 0x100
#endif

int main(void) {
    char *p1 = mmap(NULL, 4096, PROT_READ | PROT_WRITE,
                    MAP_PRIVATE | MAP_ANONYMOUS | MAP_STACK, -1, 0);
    if (p1 == MAP_FAILED) {
        printf("CONTRACT_FAIL MAP_STACK\n");
        return 1;
    }
    printf("MAP_STACK: ok\n");
    munmap(p1, 4096);

    char *p2 = mmap(NULL, 4096, PROT_READ | PROT_WRITE,
                    MAP_PRIVATE | MAP_ANONYMOUS | MAP_GROWSDOWN, -1, 0);
    if (p2 == MAP_FAILED) {
        printf("CONTRACT_FAIL MAP_GROWSDOWN\n");
        return 1;
    }
    printf("MAP_GROWSDOWN: ok\n");
    munmap(p2, 4096);

    printf("CONTRACT_PASS\n");
    return 0;
}
