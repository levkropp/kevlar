/* Contract: fork()ed child processes get scheduled and can exit.
   This is a minimal scheduling test: parent waits for child via
   waitpid(), verifying the child ran and exited cleanly. */
#define _GNU_SOURCE
#include <stdio.h>
#include <unistd.h>
#include <sys/wait.h>

int main(void) {
    pid_t child = fork();
    if (child < 0) {
        printf("CONTRACT_FAIL fork\n");
        return 1;
    }

    if (child == 0) {
        /* Child: exit with a distinctive status */
        _exit(42);
    }

    /* Parent: wait for child */
    int status;
    pid_t ret = waitpid(child, &status, 0);
    if (ret != child) {
        printf("CONTRACT_FAIL waitpid ret=%d expected=%d\n", ret, child);
        return 1;
    }

    if (!WIFEXITED(status)) {
        printf("CONTRACT_FAIL child_not_exited\n");
        return 1;
    }

    int code = WEXITSTATUS(status);
    if (code != 42) {
        printf("CONTRACT_FAIL exit_status expected=42 got=%d\n", code);
        return 1;
    }
    printf("child_scheduled: ok (exit=42)\n");

    /* Fork a second child to verify repeated scheduling */
    child = fork();
    if (child < 0) {
        printf("CONTRACT_FAIL fork2\n");
        return 1;
    }
    if (child == 0) {
        _exit(7);
    }
    waitpid(child, &status, 0);
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 7) {
        printf("CONTRACT_FAIL child2 exit=%d\n", WEXITSTATUS(status));
        return 1;
    }
    printf("child2_scheduled: ok (exit=7)\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
