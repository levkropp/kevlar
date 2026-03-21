/* Contract: setsid creates new session; second call fails EPERM. */
#include <errno.h>
#include <stdio.h>
#include <sys/wait.h>
#include <unistd.h>

int main(void) {
    pid_t child = fork();
    if (child < 0) {
        printf("CONTRACT_FAIL fork: errno=%d\n", errno);
        return 1;
    }

    if (child == 0) {
        /* Child: setsid succeeds, returns our pid */
        pid_t sid = setsid();
        pid_t mypid = getpid();
        if (sid != mypid) {
            printf("CONTRACT_FAIL setsid: sid=%d pid=%d errno=%d\n", sid, mypid, errno);
            fflush(stdout);
            _exit(1);
        }
        printf("setsid: ok\n");

        /* getsid(0) should equal our pid */
        pid_t gs = getsid(0);
        if (gs != mypid) {
            printf("CONTRACT_FAIL getsid: gs=%d pid=%d\n", gs, mypid);
            fflush(stdout);
            _exit(1);
        }
        printf("getsid: ok\n");

        /* Second setsid → EPERM (already session leader) */
        errno = 0;
        sid = setsid();
        if (sid != -1 || errno != EPERM) {
            printf("CONTRACT_FAIL setsid_eperm: sid=%d errno=%d\n", sid, errno);
            fflush(stdout);
            _exit(1);
        }
        printf("setsid_eperm: ok\n");

        fflush(stdout);
        _exit(0);
    }

    /* Parent: wait for child */
    int status;
    waitpid(child, &status, 0);
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        printf("CONTRACT_FAIL child_exit: code=%d\n", WEXITSTATUS(status));
        return 1;
    }

    printf("CONTRACT_PASS\n");
    return 0;
}
