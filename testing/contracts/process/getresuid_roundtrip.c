/* Contract: getresuid/getresgid return real/effective/saved IDs;
 * setresuid/setresgid with -1 leaves that ID unchanged. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <unistd.h>
#include <sys/types.h>

int main(void) {
    uid_t ruid, euid, suid;
    gid_t rgid, egid, sgid;

    /* getresuid */
    int ret = getresuid(&ruid, &euid, &suid);
    if (ret != 0) {
        printf("CONTRACT_FAIL getresuid: errno=%d\n", errno);
        return 1;
    }
    printf("getresuid: ok\n");

    /* getresgid */
    ret = getresgid(&rgid, &egid, &sgid);
    if (ret != 0) {
        printf("CONTRACT_FAIL getresgid: errno=%d\n", errno);
        return 1;
    }
    printf("getresgid: ok\n");

    /* setresuid with -1 = no change */
    ret = setresuid(-1, -1, -1);
    if (ret != 0) {
        printf("CONTRACT_FAIL setresuid_nop: errno=%d\n", errno);
        return 1;
    }
    uid_t ruid2, euid2, suid2;
    getresuid(&ruid2, &euid2, &suid2);
    if (ruid2 != ruid || euid2 != euid || suid2 != suid) {
        printf("CONTRACT_FAIL nop_changed: r=%d->%d e=%d->%d s=%d->%d\n",
               ruid, ruid2, euid, euid2, suid, suid2);
        return 1;
    }
    printf("setresuid_nop: ok\n");

    /* setresgid with -1 = no change */
    ret = setresgid(-1, -1, -1);
    if (ret != 0) {
        printf("CONTRACT_FAIL setresgid_nop: errno=%d\n", errno);
        return 1;
    }
    gid_t rgid2, egid2, sgid2;
    getresgid(&rgid2, &egid2, &sgid2);
    if (rgid2 != rgid || egid2 != egid || sgid2 != sgid) {
        printf("CONTRACT_FAIL gid_nop_changed\n");
        return 1;
    }
    printf("setresgid_nop: ok\n");

    /* Set euid to current ruid (always permitted), verify others unchanged */
    ret = setresuid(-1, ruid, -1);
    if (ret != 0) {
        printf("CONTRACT_FAIL set_euid: errno=%d\n", errno);
        return 1;
    }
    getresuid(&ruid2, &euid2, &suid2);
    if (ruid2 != ruid || euid2 != ruid || suid2 != suid) {
        printf("CONTRACT_FAIL set_euid_verify: r=%d e=%d s=%d\n",
               ruid2, euid2, suid2);
        return 1;
    }
    printf("set_euid: ok\n");

    /* Restore original */
    setresuid(ruid, euid, suid);

    printf("CONTRACT_PASS\n");
    return 0;
}
