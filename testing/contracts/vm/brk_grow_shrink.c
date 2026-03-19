/* Contract: brk grows/shrinks heap; new region zeroed and writable;
 * brk(NULL) returns current break. Uses raw syscall for musl compat. */
#define _GNU_SOURCE
#include <stdio.h>
#include <sys/syscall.h>
#include <unistd.h>

static void *sys_brk(void *addr) {
    return (void *)syscall(SYS_brk, addr);
}

int main(void) {
    /* brk(0) returns current break */
    void *cur = sys_brk(0);
    if (cur == (void *)-1 || cur == 0) {
        printf("CONTRACT_FAIL brk0: %p\n", cur);
        return 1;
    }
    printf("initial_brk: ok\n");

    /* Grow by 4096 */
    void *target = (char *)cur + 4096;
    void *result = sys_brk(target);
    if (result != target) {
        printf("CONTRACT_FAIL grow: target=%p result=%p\n", target, result);
        return 1;
    }
    printf("grow: ok\n");

    /* New region should be zeroed */
    char *p = (char *)cur;
    int nonzero = 0;
    for (int i = 0; i < 4096; i++) {
        if (p[i] != 0) {
            nonzero = 1;
            break;
        }
    }
    if (nonzero) {
        printf("CONTRACT_FAIL zeroed\n");
        return 1;
    }
    printf("zeroed: ok\n");

    /* Writable */
    p[0] = 42;
    p[4095] = 99;
    if (p[0] != 42 || p[4095] != 99) {
        printf("CONTRACT_FAIL writable\n");
        return 1;
    }
    printf("writable: ok\n");

    /* Shrink back */
    result = sys_brk(cur);
    if (result != cur) {
        printf("CONTRACT_FAIL shrink: expected=%p got=%p\n", cur, result);
        return 1;
    }
    printf("shrink: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
