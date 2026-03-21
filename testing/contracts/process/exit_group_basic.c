/* Contract: exit_group terminates the process with the given status;
 * parent receives exit code via wait. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>

int main(void) {
    pid_t child = fork();
    if (child < 0) {
        printf("CONTRACT_FAIL fork: errno=%d\n", errno);
        return 1;
    }

    if (child == 0) {
        /* Child: exit via exit_group with status 42 */
        syscall(SYS_exit_group, 42);
        /* Should not reach here */
        _exit(99);
    }

    /* Parent: wait for child */
    int status;
    pid_t waited = waitpid(child, &status, 0);
    if (waited != child) {
        printf("CONTRACT_FAIL waitpid: errno=%d\n", errno);
        return 1;
    }

    if (!WIFEXITED(status) || WEXITSTATUS(status) != 42) {
        printf("CONTRACT_FAIL exit_status: exited=%d code=%d\n",
               WIFEXITED(status), WEXITSTATUS(status));
        return 1;
    }
    printf("exit_group: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
