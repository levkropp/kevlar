/* Contract: getpgrp returns current process group; matches getpgid(0);
 * initial pgrp equals pid for session leader. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <unistd.h>

int main(void) {
    pid_t pgrp = getpgrp();
    if (pgrp < 0) {
        printf("CONTRACT_FAIL getpgrp: errno=%d\n", errno);
        return 1;
    }
    printf("getpgrp: ok\n");

    /* getpgid(0) should return the same value */
    pid_t pgid = getpgid(0);
    if (pgid != pgrp) {
        printf("CONTRACT_FAIL pgid_match: getpgrp=%d getpgid=%d\n", pgrp, pgid);
        return 1;
    }
    printf("pgid_match: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
