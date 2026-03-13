/* Contract: sigprocmask(SIG_BLOCK) prevents signal delivery;
 * sigprocmask(SIG_UNBLOCK) causes pending signal to be delivered. */
#include <signal.h>
#include <stdio.h>
#include <unistd.h>

static volatile int delivered = 0;

static void handler(int sig) {
    (void)sig;
    delivered++;
}

int main(void) {
    struct sigaction sa = {0};
    sa.sa_handler = handler;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGUSR1, &sa, NULL);

    /* Block SIGUSR1 */
    sigset_t set;
    sigemptyset(&set);
    sigaddset(&set, SIGUSR1);
    sigprocmask(SIG_BLOCK, &set, NULL);

    /* Send it — must not arrive yet */
    kill(getpid(), SIGUSR1);
    if (delivered != 0) {
        printf("CONTRACT_FAIL signal_while_blocked: delivered=%d\n", delivered);
        return 1;
    }
    printf("blocked_ok: delivered=%d\n", delivered);

    /* Unblock — must arrive now */
    sigprocmask(SIG_UNBLOCK, &set, NULL);
    if (delivered != 1) {
        printf("CONTRACT_FAIL signal_after_unblock: delivered=%d\n", delivered);
        return 1;
    }
    printf("unblocked_ok: delivered=%d\n", delivered);
    printf("CONTRACT_PASS\n");
    return 0;
}
