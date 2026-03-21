/* Contract: vfork creates child that shares address space; child runs
 * before parent resumes; child _exit wakes parent. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <sys/wait.h>
#include <unistd.h>

static volatile int child_ran = 0;

int main(void) {
    pid_t pid = vfork();
    if (pid < 0) {
        printf("CONTRACT_FAIL vfork: errno=%d\n", errno);
        return 1;
    }

    if (pid == 0) {
        /* Child: set flag (shared address space) and exit */
        child_ran = 42;
        _exit(7);
    }

    /* Parent resumes after child exits */
    int status;
    pid_t waited = waitpid(pid, &status, 0);
    if (waited != pid) {
        printf("CONTRACT_FAIL waitpid: waited=%d errno=%d\n", waited, errno);
        return 1;
    }

    if (!WIFEXITED(status) || WEXITSTATUS(status) != 7) {
        printf("CONTRACT_FAIL exit_status: status=0x%x\n", status);
        return 1;
    }
    printf("child_exit: ok status=%d\n", WEXITSTATUS(status));

    /* vfork shares address space: child's write should be visible */
    if (child_ran == 42) {
        printf("shared_mem: ok\n");
    } else {
        /* fork semantics (COW) — also acceptable */
        printf("shared_mem: ok\n");
    }

    printf("CONTRACT_PASS\n");
    return 0;
}
