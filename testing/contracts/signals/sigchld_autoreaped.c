/* Contract: SIGCHLD handler fires on child exit; SIG_IGN auto-reaps (no zombie). */
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <sys/wait.h>
#include <unistd.h>

static volatile int sigchld_flag = 0;

static void sigchld_handler(int sig) {
    (void)sig;
    sigchld_flag = 1;
}

int main(void) {
    /* Install SIGCHLD handler */
    struct sigaction sa = {0};
    sa.sa_handler = sigchld_handler;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGCHLD, &sa, NULL);

    /* Block SIGCHLD, fork child that exits immediately */
    sigset_t mask, old;
    sigemptyset(&mask);
    sigaddset(&mask, SIGCHLD);
    sigprocmask(SIG_BLOCK, &mask, &old);

    pid_t child = fork();
    if (child < 0) {
        printf("CONTRACT_FAIL fork: errno=%d\n", errno);
        return 1;
    }
    if (child == 0) {
        _exit(0);
    }

    /* sigsuspend with SIGCHLD unblocked → should wake on SIGCHLD */
    sigset_t empty;
    sigemptyset(&empty);
    sigsuspend(&empty);

    if (sigchld_flag != 1) {
        printf("CONTRACT_FAIL sigchld_flag: got=%d\n", sigchld_flag);
        return 1;
    }
    printf("sigchld_handler: ok flag=%d\n", sigchld_flag);

    /* Reap the child normally */
    waitpid(child, NULL, 0);

    /* Restore signal mask */
    sigprocmask(SIG_SETMASK, &old, NULL);

    /* Now set SIGCHLD to SIG_IGN → auto-reap */
    sa.sa_handler = SIG_IGN;
    sigaction(SIGCHLD, &sa, NULL);

    child = fork();
    if (child < 0) {
        printf("CONTRACT_FAIL fork2: errno=%d\n", errno);
        return 1;
    }
    if (child == 0) {
        _exit(0);
    }

    /* Brief sleep to let child exit and be auto-reaped */
    usleep(50000);

    /* wait4 should fail with ECHILD (child already auto-reaped) */
    errno = 0;
    pid_t ret = wait4(-1, NULL, WNOHANG, NULL);
    if (ret != -1 || errno != ECHILD) {
        printf("CONTRACT_FAIL autoreaped: ret=%d errno=%d expected ECHILD\n", ret, errno);
        return 1;
    }
    printf("autoreaped: ok errno=ECHILD\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
