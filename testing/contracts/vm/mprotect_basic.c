/* Contract: mprotect() changes page permissions. PROT_NONE read triggers SIGSEGV.
 * After raising to PROT_READ the memory becomes accessible again. */
#include <signal.h>
#include <setjmp.h>
#include <stdio.h>
#include <sys/mman.h>
#include <unistd.h>

static jmp_buf jb;
static int fault_count = 0;

static void segv_handler(int sig) {
    (void)sig;
    fault_count++;
    longjmp(jb, 1);
}

int main(void) {
    char *p = mmap(NULL, 4096, PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == MAP_FAILED) {
        printf("CONTRACT_FAIL mmap\n");
        return 1;
    }
    p[0] = 42;

    signal(SIGSEGV, segv_handler);

    /* Protect: read should fault */
    mprotect(p, 4096, PROT_NONE);
    if (setjmp(jb) == 0) {
        volatile char x = p[0]; /* expected fault */
        (void)x;
        printf("CONTRACT_FAIL prot_none: read did not fault\n");
        return 1;
    }
    if (fault_count != 1) {
        printf("CONTRACT_FAIL fault_count=%d\n", fault_count);
        return 1;
    }
    printf("prot_none_faulted: ok\n");

    /* Restore read access */
    mprotect(p, 4096, PROT_READ);
    volatile char y = p[0];
    if (y != 42) {
        printf("CONTRACT_FAIL prot_read: got %d expected 42\n", (int)y);
        return 1;
    }
    printf("prot_read_ok: %d\n", (int)y);
    printf("CONTRACT_PASS\n");
    munmap(p, 4096);
    return 0;
}
