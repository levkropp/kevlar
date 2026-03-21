/* Contract: getgroups returns supplementary group count; with size=0
 * returns count without writing; result >= 0. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <unistd.h>

int main(void) {
    /* Query count with size=0 */
    int count = getgroups(0, NULL);
    if (count < 0) {
        printf("CONTRACT_FAIL count: ret=%d errno=%d\n", count, errno);
        return 1;
    }
    printf("count: ok\n");

    /* Get actual groups if any */
    if (count > 0) {
        gid_t groups[64];
        int n = getgroups(count < 64 ? count : 64, groups);
        if (n < 0) {
            printf("CONTRACT_FAIL getgroups: ret=%d errno=%d\n", n, errno);
            return 1;
        }
    }
    printf("getgroups: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
