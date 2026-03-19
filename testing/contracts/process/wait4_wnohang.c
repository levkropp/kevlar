/* Contract: wait4 WNOHANG returns 0 when child still running. */
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

int main(void) {
    pid_t child = fork();
    if (child < 0) {
        printf("CONTRACT_FAIL fork: errno=%d\n", errno);
        return 1;
    }

    if (child == 0) {
        /* Child: sleep 100ms then exit 7 */
        struct timespec ts = {0, 100000000};
        nanosleep(&ts, NULL);
        _exit(7);
    }

    /* Parent: WNOHANG should return 0 immediately */
    int status = 0;
    pid_t ret = wait4(-1, &status, WNOHANG, NULL);
    if (ret != 0) {
        printf("CONTRACT_FAIL wnohang: ret=%d expected=0 errno=%d\n", ret, errno);
        return 1;
    }
    printf("wnohang: ok ret=0\n");

    /* Blocking wait should return child pid with exit code 7 */
    ret = wait4(-1, &status, 0, NULL);
    if (ret != child) {
        printf("CONTRACT_FAIL wait_child: ret=%d expected=%d errno=%d\n", ret, child, errno);
        return 1;
    }
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 7) {
        printf("CONTRACT_FAIL exit_status: exited=%d code=%d\n",
               WIFEXITED(status), WEXITSTATUS(status));
        return 1;
    }
    printf("wait_blocking: ok pid=%d exit=%d\n", ret, WEXITSTATUS(status));

    printf("CONTRACT_PASS\n");
    return 0;
}
