/* Contract: sethostname/setdomainname change the UTS fields; uname
 * reflects the change. Requires root or EPERM on non-root. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/utsname.h>
#include <unistd.h>

int main(void) {
    /* Save original */
    struct utsname orig;
    uname(&orig);

    /* sethostname */
    const char *newhost = "kevlar-test";
    int ret = sethostname(newhost, strlen(newhost));
    if (ret == 0) {
        struct utsname u;
        uname(&u);
        if (strcmp(u.nodename, newhost) != 0) {
            printf("CONTRACT_FAIL hostname_verify: got=%s\n", u.nodename);
            return 1;
        }
        printf("sethostname: ok\n");

        /* Restore */
        sethostname(orig.nodename, strlen(orig.nodename));
    } else if (errno == EPERM) {
        printf("sethostname: ok\n");
    } else {
        printf("CONTRACT_FAIL sethostname: errno=%d\n", errno);
        return 1;
    }

    /* setdomainname */
    const char *newdom = "test.local";
    ret = setdomainname(newdom, strlen(newdom));
    if (ret == 0) {
        struct utsname u;
        uname(&u);
        if (strcmp(u.domainname, newdom) != 0) {
            printf("CONTRACT_FAIL domain_verify: got=%s\n", u.domainname);
            return 1;
        }
        printf("setdomainname: ok\n");

        /* Restore */
        setdomainname(orig.domainname, strlen(orig.domainname));
    } else if (errno == EPERM) {
        printf("setdomainname: ok\n");
    } else {
        printf("CONTRACT_FAIL setdomainname: errno=%d\n", errno);
        return 1;
    }

    printf("CONTRACT_PASS\n");
    return 0;
}
