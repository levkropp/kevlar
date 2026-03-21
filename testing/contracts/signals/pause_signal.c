/* Contract: pause() blocks until signal arrives, then returns -1 with
 * errno=EINTR. */
#define _GNU_SOURCE
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <sys/wait.h>
#include <unistd.h>

static volatile int handler_called = 0;

static void handler(int sig) {
    (void)sig;
    handler_called = 1;
}

int main(void) {
    /* Use fork: child sends signal to parent after brief delay */
    struct sigaction sa = {0};
    sa.sa_handler = handler;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGUSR1, &sa, NULL);

    pid_t child = fork();
    if (child < 0) {
        printf("CONTRACT_FAIL fork: errno=%d\n", errno);
        return 1;
    }

    if (child == 0) {
        /* Child: small delay then signal parent */
        usleep(10000); /* 10ms */
        kill(getppid(), SIGUSR1);
        _exit(0);
    }

    /* Parent: pause waits for signal */
    int ret = pause();
    if (ret != -1 || errno != EINTR) {
        printf("CONTRACT_FAIL pause: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    if (!handler_called) {
        printf("CONTRACT_FAIL handler_not_called\n");
        return 1;
    }
    printf("pause: ok\n");

    /* Reap child */
    waitpid(child, NULL, 0);

    printf("CONTRACT_PASS\n");
    return 0;
}
