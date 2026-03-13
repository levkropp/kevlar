/* Contract: getpriority()/setpriority() read and write process nice value.
 * Default nice is 0; setpriority to 5 must be reflected by getpriority. */
#include <errno.h>
#include <stdio.h>
#include <sys/resource.h>

int main(void) {
    errno = 0;
    int prio = getpriority(PRIO_PROCESS, 0);
    if (errno != 0) {
        printf("CONTRACT_FAIL getpriority_default: errno=%d\n", errno);
        return 1;
    }
    printf("default_priority: %d\n", prio);

    if (setpriority(PRIO_PROCESS, 0, 5) != 0) {
        printf("CONTRACT_FAIL setpriority: errno=%d\n", errno);
        return 1;
    }

    errno = 0;
    int new_prio = getpriority(PRIO_PROCESS, 0);
    if (errno != 0) {
        printf("CONTRACT_FAIL getpriority_after_set: errno=%d\n", errno);
        return 1;
    }
    if (new_prio != 5) {
        printf("CONTRACT_FAIL priority_mismatch: got=%d expected=5\n", new_prio);
        return 1;
    }
    printf("set_priority: %d\n", new_prio);
    printf("CONTRACT_PASS\n");
    return 0;
}
