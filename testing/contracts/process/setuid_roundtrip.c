/* Contract: setuid(getuid()) and setgid(getgid()) are no-op roundtrips. */
#include <errno.h>
#include <stdio.h>
#include <unistd.h>

int main(void) {
    uid_t uid = getuid();
    if (setuid(uid) != 0) {
        printf("CONTRACT_FAIL setuid: errno=%d\n", errno);
        return 1;
    }
    if (getuid() != uid) {
        printf("CONTRACT_FAIL uid_changed: got=%d expected=%d\n", getuid(), uid);
        return 1;
    }
    printf("setuid_roundtrip: ok uid=%d\n", uid);

    gid_t gid = getgid();
    if (setgid(gid) != 0) {
        printf("CONTRACT_FAIL setgid: errno=%d\n", errno);
        return 1;
    }
    if (getgid() != gid) {
        printf("CONTRACT_FAIL gid_changed: got=%d expected=%d\n", getgid(), gid);
        return 1;
    }
    printf("setgid_roundtrip: ok gid=%d\n", gid);

    printf("CONTRACT_PASS\n");
    return 0;
}
