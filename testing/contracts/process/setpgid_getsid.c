/* Contract: setpgid creates process group; getsid returns session;
 * setsid from non-leader creates new session. */
#include <errno.h>
#include <stdio.h>
#include <sys/wait.h>
#include <unistd.h>

int main(void) {
    pid_t pid = getpid();
    pid_t pgid = getpgid(0);

    /* getpgid(0) returns current pgid */
    if (pgid < 0) {
        printf("CONTRACT_FAIL getpgid: errno=%d\n", errno);
        return 1;
    }
    printf("getpgid: ok\n");

    /* getsid(0) returns session id */
    pid_t sid = getsid(0);
    if (sid < 0) {
        printf("CONTRACT_FAIL getsid: errno=%d\n", errno);
        return 1;
    }
    printf("getsid: ok\n");

    /* Fork child that creates new session.
     * Child signals results via exit code to avoid _exit stdio flush issues. */
    pid_t child = fork();
    if (child == 0) {
        pid_t new_sid = setsid();
        if (new_sid == -1) _exit(1);
        if (new_sid != getpid()) _exit(2);
        pid_t new_pgid = getpgid(0);
        if (new_pgid != getpid()) _exit(3);
        _exit(0);
    }

    int status;
    waitpid(child, &status, 0);
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        printf("CONTRACT_FAIL setsid_child: exit=%d\n", WEXITSTATUS(status));
        return 1;
    }
    printf("setsid: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
