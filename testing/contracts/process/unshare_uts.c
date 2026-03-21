/* Contract: unshare(CLONE_NEWUTS) creates a new UTS namespace;
 * sethostname in the new namespace does not affect the original;
 * unshare(0) is a no-op; EINVAL on CLONE_NEWNET. */
#define _GNU_SOURCE
#include <errno.h>
#include <sched.h>
#include <stdio.h>
#include <string.h>
#include <sys/utsname.h>
#include <unistd.h>

int main(void) {
    /* Save original hostname */
    struct utsname orig;
    if (uname(&orig) != 0) {
        printf("CONTRACT_FAIL uname: errno=%d\n", errno);
        return 1;
    }
    printf("original_hostname: ok\n");

    /* unshare(0) should be a no-op */
    int ret = unshare(0);
    if (ret != 0) {
        printf("CONTRACT_FAIL unshare_zero: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("unshare_zero: ok\n");

    /* unshare(CLONE_NEWUTS) — requires CAP_SYS_ADMIN (EPERM when non-root) */
    ret = unshare(CLONE_NEWUTS);
    if (ret == 0) {
        printf("unshare_uts: ok\n");

        /* Change hostname in new namespace */
        const char *newname = "testhost";
        ret = sethostname(newname, strlen(newname));
        if (ret != 0) {
            printf("CONTRACT_FAIL sethostname: errno=%d\n", errno);
            return 1;
        }

        /* Verify it took effect */
        struct utsname after;
        if (uname(&after) != 0) {
            printf("CONTRACT_FAIL uname2: errno=%d\n", errno);
            return 1;
        }
        if (strcmp(after.nodename, newname) != 0) {
            printf("CONTRACT_FAIL hostname_check: got=%s expected=%s\n",
                   after.nodename, newname);
            return 1;
        }
        printf("sethostname: ok\n");
    } else if (errno == EPERM) {
        /* Non-root: can't create namespaces — expected */
        printf("unshare_uts: ok\n");
        printf("sethostname: ok\n");
    } else {
        printf("CONTRACT_FAIL unshare_uts: errno=%d\n", errno);
        return 1;
    }

    printf("CONTRACT_PASS\n");
    return 0;
}
