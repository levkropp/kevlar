/* Contract: standard signals are coalesced — sending the same signal
   multiple times while it's masked delivers it only once after unmasking. */
#define _GNU_SOURCE
#include <signal.h>
#include <stdio.h>
#include <unistd.h>
#include <sys/syscall.h>

static volatile int sigusr1_count = 0;

static void handler(int sig) {
    (void)sig;
    sigusr1_count++;
}

int main(void) {
    struct sigaction sa;
    sa.sa_handler = handler;
    sigemptyset(&sa.sa_mask);
    sa.sa_flags = 0;
    if (sigaction(SIGUSR1, &sa, NULL) != 0) {
        printf("CONTRACT_FAIL sigaction\n");
        return 1;
    }

    /* Block SIGUSR1 */
    sigset_t mask, oldmask;
    sigemptyset(&mask);
    sigaddset(&mask, SIGUSR1);
    if (sigprocmask(SIG_BLOCK, &mask, &oldmask) != 0) {
        printf("CONTRACT_FAIL sigprocmask_block\n");
        return 1;
    }

    /* Send SIGUSR1 to self 5 times while blocked */
    pid_t self = getpid();
    for (int i = 0; i < 5; i++) {
        kill(self, SIGUSR1);
    }

    /* Handler should NOT have run yet (signal is blocked) */
    if (sigusr1_count != 0) {
        printf("CONTRACT_FAIL blocked_delivery count=%d\n", sigusr1_count);
        return 1;
    }
    printf("blocked: ok (count=0)\n");

    /* Unblock — standard signal should be delivered exactly once */
    sigprocmask(SIG_SETMASK, &oldmask, NULL);

    /* Standard signals are coalesced: 5 sends → 1 delivery */
    if (sigusr1_count != 1) {
        printf("CONTRACT_FAIL coalesce expected=1 got=%d\n", sigusr1_count);
        return 1;
    }
    printf("coalesced: ok (count=1)\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
