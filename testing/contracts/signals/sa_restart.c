/* Contract: without SA_RESTART, a signal-interrupted read() returns EINTR.
 * We use SIGALRM (self-signal via alarm) to avoid race conditions with fork. */
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <sys/time.h>
#include <unistd.h>

static volatile int handler_ran = 0;

static void handler(int sig) {
    (void)sig;
    handler_ran = 1;
}

int main(void) {
    /* Install SIGALRM handler WITHOUT SA_RESTART */
    struct sigaction sa = {0};
    sa.sa_handler = handler;
    sigemptyset(&sa.sa_mask);
    sa.sa_flags = 0; /* no SA_RESTART */
    sigaction(SIGALRM, &sa, NULL);

    int pipefd[2];
    if (pipe(pipefd) < 0) {
        printf("CONTRACT_FAIL pipe\n");
        return 1;
    }

    /* Fire SIGALRM in 100ms */
    struct itimerval itv = {{0, 0}, {0, 100000}};
    setitimer(ITIMER_REAL, &itv, NULL);

    /* Blocking read on empty pipe — should be interrupted by SIGALRM */
    char buf[1];
    int ret = (int)read(pipefd[0], buf, 1);
    int saved_errno = errno;

    close(pipefd[0]);
    close(pipefd[1]);

    if (ret == -1 && saved_errno == EINTR && handler_ran) {
        printf("eintr_ok: ret=%d errno=EINTR handler_ran=%d\n",
               ret, handler_ran);
        printf("CONTRACT_PASS\n");
        return 0;
    }
    printf("CONTRACT_FAIL eintr: ret=%d errno=%d handler_ran=%d\n",
           ret, saved_errno, handler_ran);
    return 1;
}
