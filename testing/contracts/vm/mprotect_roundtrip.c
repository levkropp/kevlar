/* Contract: mprotect RW→RO write faults; RO→RW write succeeds. */
#include <setjmp.h>
#include <signal.h>
#include <stdio.h>
#include <sys/mman.h>

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
    p[0] = 'A';
    printf("initial_write: ok\n");

    signal(SIGSEGV, segv_handler);

    /* RW → RO: write should fault */
    mprotect(p, 4096, PROT_READ);
    if (setjmp(jb) == 0) {
        p[0] = 'B';
        printf("CONTRACT_FAIL ro_write: no fault\n");
        return 1;
    }
    if (fault_count != 1) {
        printf("CONTRACT_FAIL ro_fault_count: %d\n", fault_count);
        return 1;
    }
    /* Read still works */
    if (p[0] != 'A') {
        printf("CONTRACT_FAIL ro_read: got=%c\n", p[0]);
        return 1;
    }
    printf("ro_faults: ok\n");

    /* RO → RW: write should succeed */
    mprotect(p, 4096, PROT_READ | PROT_WRITE);
    p[0] = 'B';
    if (p[0] != 'B') {
        printf("CONTRACT_FAIL rw_restore: got=%c\n", p[0]);
        return 1;
    }
    printf("rw_restore: ok\n");

    munmap(p, 4096);
    printf("CONTRACT_PASS\n");
    return 0;
}
