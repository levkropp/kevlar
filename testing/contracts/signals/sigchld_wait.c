/* Contract: SIGCHLD delivered on child exit; wait4 reaps;
 * WNOHANG returns 0 when no waitable child. */
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <sys/wait.h>
#include <unistd.h>

static volatile int sigchld_count = 0;

static void sigchld_handler(int sig) {
    (void)sig;
    sigchld_count++;
}

int main(void) {
    struct sigaction sa = {0};
    sa.sa_handler = sigchld_handler;
    sa.sa_flags = SA_NOCLDSTOP;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGCHLD, &sa, NULL);

    /* WNOHANG with no children → -1/ECHILD */
    errno = 0;
    pid_t w = waitpid(-1, NULL, WNOHANG);
    if (w != -1 || errno != ECHILD) {
        printf("CONTRACT_FAIL no_children: w=%d errno=%d\n", w, errno);
        return 1;
    }
    printf("no_children: ok\n");

    /* Fork child that exits 42 */
    pid_t pid = fork();
    if (pid == 0) {
        _exit(42);
    }

    /* Wait for child (retry on EINTR from SIGCHLD handler) */
    int status;
    do {
        w = waitpid(pid, &status, 0);
    } while (w == -1 && errno == EINTR);
    if (w != pid) {
        printf("CONTRACT_FAIL waitpid: w=%d expected=%d errno=%d\n", w, pid, errno);
        return 1;
    }
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 42) {
        printf("CONTRACT_FAIL exit_status: status=0x%x\n", status);
        return 1;
    }
    printf("exit_status: ok (42)\n");

    /* SIGCHLD should have been delivered */
    if (sigchld_count < 1) {
        printf("CONTRACT_FAIL sigchld: count=%d\n", sigchld_count);
        return 1;
    }
    printf("sigchld_delivered: ok count=%d\n", sigchld_count);

    printf("CONTRACT_PASS\n");
    return 0;
}
