/* Contract: getuid/geteuid/getgid/getegid return consistent values;
 * survive fork. */
#include <errno.h>
#include <stdio.h>
#include <sys/wait.h>
#include <unistd.h>

int main(void) {
    uid_t uid = getuid();
    uid_t euid = geteuid();
    gid_t gid = getgid();
    gid_t egid = getegid();

    /* uid == euid (no setuid) */
    if (uid != euid) {
        printf("CONTRACT_FAIL uid_euid: uid=%d euid=%d\n", uid, euid);
        return 1;
    }
    printf("uid_euid_match: ok\n");

    /* gid == egid (no setgid) */
    if (gid != egid) {
        printf("CONTRACT_FAIL gid_egid: gid=%d egid=%d\n", gid, egid);
        return 1;
    }
    printf("gid_egid_match: ok\n");

    /* Values survive fork */
    pid_t child = fork();
    if (child == 0) {
        if (getuid() != uid || geteuid() != euid ||
            getgid() != gid || getegid() != egid) {
            /* Signal failure via exit code — avoid printf in child
             * since _exit doesn't flush stdio buffers. */
            _exit(1);
        }
        _exit(0);
    }

    int status;
    waitpid(child, &status, 0);
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        printf("CONTRACT_FAIL fork_ids: status=0x%x\n", status);
        return 1;
    }
    printf("fork_ids: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
