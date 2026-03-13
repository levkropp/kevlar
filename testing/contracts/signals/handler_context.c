/* Contract: sigaction() registers a handler; signal delivery invokes it
   with the correct signal number.  Also tests that the old handler
   is returned correctly. */
#define _GNU_SOURCE
#include <signal.h>
#include <stdio.h>
#include <unistd.h>

static volatile int received_signal = 0;

static void handler(int sig) {
    received_signal = sig;
}

int main(void) {
    /* Install handler for SIGUSR2 */
    struct sigaction sa, old_sa;
    sa.sa_handler = handler;
    sigemptyset(&sa.sa_mask);
    sa.sa_flags = 0;

    if (sigaction(SIGUSR2, &sa, &old_sa) != 0) {
        printf("CONTRACT_FAIL sigaction\n");
        return 1;
    }

    /* Old handler should have been SIG_DFL */
    if (old_sa.sa_handler != SIG_DFL) {
        printf("CONTRACT_FAIL old_handler not SIG_DFL\n");
        return 1;
    }
    printf("old_handler: SIG_DFL ok\n");

    /* Send SIGUSR2 to self */
    kill(getpid(), SIGUSR2);

    /* Handler should have been called with SIGUSR2 */
    if (received_signal != SIGUSR2) {
        printf("CONTRACT_FAIL received=%d expected=%d\n",
               received_signal, SIGUSR2);
        return 1;
    }
    printf("handler_called: ok (sig=%d)\n", received_signal);

    /* Replace handler and verify old one is returned */
    struct sigaction sa2, old_sa2;
    sa2.sa_handler = SIG_IGN;
    sigemptyset(&sa2.sa_mask);
    sa2.sa_flags = 0;

    if (sigaction(SIGUSR2, &sa2, &old_sa2) != 0) {
        printf("CONTRACT_FAIL sigaction2\n");
        return 1;
    }

    if (old_sa2.sa_handler != handler) {
        printf("CONTRACT_FAIL old_handler2 not handler\n");
        return 1;
    }
    printf("replace_handler: ok\n");

    /* Sending SIGUSR2 now should be ignored (SIG_IGN) */
    received_signal = 0;
    kill(getpid(), SIGUSR2);
    if (received_signal != 0) {
        printf("CONTRACT_FAIL sig_ign_delivered\n");
        return 1;
    }
    printf("sig_ign: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
