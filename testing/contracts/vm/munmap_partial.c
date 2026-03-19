/* Contract: partial munmap splits VMAs; first/last still accessible;
 * middle page faults with SIGSEGV. */
#include <setjmp.h>
#include <signal.h>
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
    long pagesz = sysconf(_SC_PAGESIZE);

    /* Map 3 contiguous pages */
    char *base = mmap(NULL, 3 * pagesz, PROT_READ | PROT_WRITE,
                      MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (base == MAP_FAILED) {
        printf("CONTRACT_FAIL mmap: base=MAP_FAILED\n");
        return 1;
    }

    /* Write markers */
    base[0] = 'A';
    base[pagesz] = 'B';
    base[2 * pagesz] = 'C';

    /* Unmap middle page */
    munmap(base + pagesz, pagesz);

    /* First page still accessible */
    if (base[0] != 'A') {
        printf("CONTRACT_FAIL first_page: got=%d\n", base[0]);
        return 1;
    }
    printf("first_page: ok\n");

    /* Last page still accessible */
    if (base[2 * pagesz] != 'C') {
        printf("CONTRACT_FAIL last_page: got=%d\n", base[2 * pagesz]);
        return 1;
    }
    printf("last_page: ok\n");

    /* Middle page should fault */
    signal(SIGSEGV, segv_handler);
    if (setjmp(jb) == 0) {
        volatile char x = base[pagesz];
        (void)x;
        printf("CONTRACT_FAIL middle_fault: no fault\n");
        return 1;
    }
    if (fault_count != 1) {
        printf("CONTRACT_FAIL fault_count: %d\n", fault_count);
        return 1;
    }
    printf("middle_faults: ok\n");

    munmap(base, pagesz);
    munmap(base + 2 * pagesz, pagesz);
    printf("CONTRACT_PASS\n");
    return 0;
}
