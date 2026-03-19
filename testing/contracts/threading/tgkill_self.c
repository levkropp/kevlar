/* Contract: tgkill delivers signal to calling thread (self-targeted). */
#define _GNU_SOURCE
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <unistd.h>

static volatile pid_t handler_tid = 0;

static void handler(int sig) {
    (void)sig;
    handler_tid = syscall(SYS_gettid);
}

int main(void) {
    struct sigaction sa = {0};
    sa.sa_handler = handler;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGUSR1, &sa, NULL);

    pid_t pid = getpid();
    pid_t tid = syscall(SYS_gettid);

    if (syscall(SYS_tgkill, pid, tid, SIGUSR1) != 0) {
        printf("CONTRACT_FAIL tgkill: errno=%d\n", errno);
        return 1;
    }

    if (handler_tid != tid) {
        printf("CONTRACT_FAIL handler_tid: got=%d expected=%d\n", handler_tid, tid);
        return 1;
    }
    printf("tgkill_self: ok tid=%d handler_tid=%d\n", tid, handler_tid);

    printf("CONTRACT_PASS\n");
    return 0;
}
