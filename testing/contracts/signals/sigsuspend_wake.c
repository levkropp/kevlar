/* Contract: sigsuspend atomically replaces signal mask and waits;
 * returns -1/EINTR when signal delivered; original mask restored. */
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <sys/wait.h>
#include <unistd.h>

static volatile int handler_ran = 0;

static void usr1_handler(int sig) {
    (void)sig;
    handler_ran = 1;
}

int main(void) {
    struct sigaction sa = {0};
    sa.sa_handler = usr1_handler;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGUSR1, &sa, NULL);

    /* Block SIGUSR1 */
    sigset_t block, old;
    sigemptyset(&block);
    sigaddset(&block, SIGUSR1);
    sigprocmask(SIG_BLOCK, &block, &old);

    /* Fork child to send SIGUSR1 after brief delay */
    pid_t pid = fork();
    if (pid == 0) {
        usleep(50000); /* 50ms */
        kill(getppid(), SIGUSR1);
        _exit(0);
    }

    /* sigsuspend with empty mask (allows SIGUSR1 delivery) */
    sigset_t empty;
    sigemptyset(&empty);
    errno = 0;
    int ret = sigsuspend(&empty);
    if (ret != -1 || errno != EINTR) {
        printf("CONTRACT_FAIL sigsuspend: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    if (handler_ran != 1) {
        printf("CONTRACT_FAIL handler: handler_ran=%d\n", handler_ran);
        return 1;
    }
    printf("sigsuspend: ok\n");

    /* Original mask should be restored (SIGUSR1 blocked again) */
    sigset_t cur;
    sigprocmask(SIG_BLOCK, NULL, &cur);
    if (!sigismember(&cur, SIGUSR1)) {
        printf("CONTRACT_FAIL mask_restored: SIGUSR1 not blocked\n");
        return 1;
    }
    printf("mask_restored: ok\n");

    int status;
    waitpid(pid, &status, 0);
    printf("CONTRACT_PASS\n");
    return 0;
}
