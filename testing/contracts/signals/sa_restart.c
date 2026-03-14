/* Contract: without SA_RESTART, a signal-interrupted read() returns EINTR.
 * We use fork + kill to deliver the signal to the parent while it's
 * blocked on a pipe read. */
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <unistd.h>
#include <sys/wait.h>

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

    pid_t parent_pid = getpid();
    pid_t child = fork();
    if (child < 0) {
        printf("CONTRACT_FAIL fork\n");
        return 1;
    }

    if (child == 0) {
        /* Child: wait briefly, then send SIGALRM to parent */
        close(pipefd[0]);
        close(pipefd[1]);
        /* Busy-wait ~50ms worth of iterations to give parent time to block */
        for (volatile int i = 0; i < 500000; i++) {}
        kill(parent_pid, SIGALRM);
        _exit(0);
    }

    /* Parent: blocking read on empty pipe — should be interrupted by SIGALRM */
    char buf[1];
    int ret = (int)read(pipefd[0], buf, 1);
    int saved_errno = errno;

    /* Reap child */
    waitpid(child, NULL, 0);
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
