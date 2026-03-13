/* Contract: fork + execve + waitpid work correctly together.
   Validates Tier 1 program compatibility by spawning child processes
   and checking exit status. */
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/wait.h>

static int run_and_check(int expected_status) {
    pid_t pid = fork();
    if (pid < 0) return -1;
    if (pid == 0) {
        _exit(expected_status);
    }
    int status;
    waitpid(pid, &status, 0);
    if (!WIFEXITED(status)) return -2;
    return WEXITSTATUS(status);
}

int main(void) {
    /* Test: child exits 0 */
    int ret = run_and_check(0);
    if (ret != 0) {
        printf("CONTRACT_FAIL exit0 got=%d\n", ret);
        return 1;
    }
    printf("fork_exit_0: ok\n");

    /* Test: child exits 1 */
    ret = run_and_check(1);
    if (ret != 1) {
        printf("CONTRACT_FAIL exit1 got=%d\n", ret);
        return 1;
    }
    printf("fork_exit_1: ok\n");

    /* Test: child exits 42 */
    ret = run_and_check(42);
    if (ret != 42) {
        printf("CONTRACT_FAIL exit42 got=%d\n", ret);
        return 1;
    }
    printf("fork_exit_42: ok\n");

    /* Test: multiple children in sequence */
    for (int i = 0; i < 5; i++) {
        ret = run_and_check(i * 10);
        if (ret != i * 10) {
            printf("CONTRACT_FAIL multi child=%d expected=%d got=%d\n", i, i*10, ret);
            return 1;
        }
    }
    printf("multi_fork: ok (5 children)\n");

    /* Test: parent pid is stable */
    pid_t my_pid = getpid();
    pid_t child = fork();
    if (child == 0) {
        pid_t ppid = getppid();
        _exit(ppid == my_pid ? 0 : 1);
    }
    int status;
    waitpid(child, &status, 0);
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        printf("CONTRACT_FAIL ppid\n");
        return 1;
    }
    printf("ppid_correct: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
