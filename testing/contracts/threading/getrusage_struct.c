/* Contract: getrusage returns 0 for RUSAGE_SELF and RUSAGE_CHILDREN (stub). */
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/resource.h>

int main(void) {
    struct rusage ru;
    memset(&ru, 0xFF, sizeof(ru));

    if (getrusage(RUSAGE_SELF, &ru) != 0) {
        printf("CONTRACT_FAIL rusage_self: errno=%d\n", errno);
        return 1;
    }
    /* Stub writes zeros; verify no segfault and struct is accessible */
    printf("rusage_self: ok\n");

    memset(&ru, 0xFF, sizeof(ru));
    if (getrusage(RUSAGE_CHILDREN, &ru) != 0) {
        printf("CONTRACT_FAIL rusage_children: errno=%d\n", errno);
        return 1;
    }
    printf("rusage_children: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
