/* Contract: tkill delivers signal to the calling thread (like tgkill
 * but without tgid parameter). */
#define _GNU_SOURCE
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <unistd.h>

static volatile int handler_fired = 0;

static void handler(int sig) {
    (void)sig;
    handler_fired = 1;
}

int main(void) {
    struct sigaction sa = {0};
    sa.sa_handler = handler;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGUSR1, &sa, NULL);

    pid_t tid = syscall(SYS_gettid);

    /* tkill(tid, SIGUSR1) */
    int ret = syscall(SYS_tkill, tid, SIGUSR1);
    if (ret != 0) {
        printf("CONTRACT_FAIL tkill: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    if (!handler_fired) {
        printf("CONTRACT_FAIL handler_not_fired\n");
        return 1;
    }
    printf("tkill: ok\n");

    /* tkill with invalid signal */
    errno = 0;
    ret = syscall(SYS_tkill, tid, 0);
    if (ret != 0) {
        printf("CONTRACT_FAIL signal_zero: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("signal_zero: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
