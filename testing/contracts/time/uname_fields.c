/* Contract: uname returns non-empty, null-terminated strings. */
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/utsname.h>

int main(void) {
    struct utsname u;
    if (uname(&u) != 0) {
        printf("CONTRACT_FAIL uname: errno=%d\n", errno);
        return 1;
    }

    /* sysname non-empty */
    if (strlen(u.sysname) == 0) {
        printf("CONTRACT_FAIL sysname: empty\n");
        return 1;
    }
    printf("sysname: ok\n");

    /* release non-empty */
    if (strlen(u.release) == 0) {
        printf("CONTRACT_FAIL release: empty\n");
        return 1;
    }
    printf("release: ok\n");

    /* machine non-empty */
    if (strlen(u.machine) == 0) {
        printf("CONTRACT_FAIL machine: empty\n");
        return 1;
    }
    printf("machine: ok\n");

    /* nodename non-empty */
    if (strlen(u.nodename) == 0) {
        printf("CONTRACT_FAIL nodename: empty\n");
        return 1;
    }
    printf("nodename: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
