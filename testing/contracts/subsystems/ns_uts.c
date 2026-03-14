/* Contract: UTS namespace isolation via unshare + sethostname. */
#define _GNU_SOURCE
#include <errno.h>
#include <sched.h>
#include <stdio.h>
#include <string.h>
#include <sys/utsname.h>
#include <unistd.h>

int main(void) {
    struct utsname uts;

    /* Get initial hostname */
    uname(&uts);
    char orig_host[65];
    strncpy(orig_host, uts.nodename, sizeof(orig_host));
    printf("ns_uts_initial: ok\n");

    /* unshare UTS namespace */
    int ret = unshare(CLONE_NEWUTS);
    if (ret != 0) {
        printf("CONTRACT_FAIL ns_uts_unshare: errno=%d\n", errno);
        return 1;
    }
    printf("ns_uts_unshare: ok\n");

    /* Set hostname in new namespace */
    ret = sethostname("isolated", 8);
    if (ret != 0) {
        printf("CONTRACT_FAIL ns_uts_sethostname: errno=%d\n", errno);
        return 1;
    }
    printf("ns_uts_sethostname: ok\n");

    /* Verify uname returns new hostname */
    uname(&uts);
    if (strcmp(uts.nodename, "isolated") != 0) {
        printf("CONTRACT_FAIL ns_uts_verify: got '%s'\n", uts.nodename);
        return 1;
    }
    printf("ns_uts_verify: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
