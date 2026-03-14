/* Contract: waitid returns correct siginfo_t for exited child. */
#define _GNU_SOURCE
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/wait.h>
#include <unistd.h>

int main(void) {
    /* Fork a child that exits with code 42 */
    pid_t child = fork();
    if (child < 0) {
        printf("CONTRACT_FAIL waitid_fork\n");
        return 1;
    }
    if (child == 0) {
        _exit(42);
    }

    /* waitid with P_PID */
    siginfo_t info;
    int ret = waitid(P_PID, child, &info, WEXITED);
    if (ret != 0) {
        printf("CONTRACT_FAIL waitid_call: errno=%d\n", errno);
        return 1;
    }

    /* Verify siginfo fields */
    if (info.si_pid != child) {
        printf("CONTRACT_FAIL waitid_pid: got %d expected %d\n", info.si_pid, child);
        return 1;
    }
    printf("waitid_pid: ok\n");

    if (info.si_signo != SIGCHLD) {
        printf("CONTRACT_FAIL waitid_signo: got %d expected %d\n", info.si_signo, SIGCHLD);
        return 1;
    }
    printf("waitid_signo: ok\n");

    if (info.si_code != CLD_EXITED) {
        printf("CONTRACT_FAIL waitid_code: got %d expected %d\n", info.si_code, CLD_EXITED);
        return 1;
    }
    printf("waitid_code: ok\n");

    if (info.si_status != 42) {
        printf("CONTRACT_FAIL waitid_status: got %d expected 42\n", info.si_status);
        return 1;
    }
    printf("waitid_status: ok\n");

    /* waitid with WNOHANG and no children should zero siginfo */
    siginfo_t info2 = { .si_pid = 999 };
    ret = waitid(P_ALL, 0, &info2, WEXITED | WNOHANG);
    /* On Linux, returns 0 with si_pid=0 when no waitable children.
       Some Linux versions return -1/ECHILD. Accept both. */
    if (ret == 0 && info2.si_pid == 0) {
        printf("waitid_nohang: ok\n");
    } else if (ret == -1 && errno == ECHILD) {
        printf("waitid_nohang: ok\n");
    } else {
        printf("CONTRACT_FAIL waitid_nohang: ret=%d si_pid=%d errno=%d\n",
               ret, info2.si_pid, errno);
        return 1;
    }

    printf("CONTRACT_PASS\n");
    return 0;
}
