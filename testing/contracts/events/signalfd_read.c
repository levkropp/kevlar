/* Contract: signalfd reads signalfd_siginfo struct;
 * EAGAIN when empty (SFD_NONBLOCK). */
#define _GNU_SOURCE
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <sys/signalfd.h>
#include <unistd.h>

int main(void) {
    sigset_t mask;
    sigemptyset(&mask);
    sigaddset(&mask, SIGUSR1);

    /* Block SIGUSR1 so it doesn't kill us */
    sigprocmask(SIG_BLOCK, &mask, NULL);

    int sfd = signalfd(-1, &mask, SFD_NONBLOCK);
    if (sfd < 0) {
        printf("CONTRACT_FAIL signalfd: errno=%d\n", errno);
        return 1;
    }

    /* Read with no pending signal → EAGAIN */
    struct signalfd_siginfo info;
    errno = 0;
    ssize_t n = read(sfd, &info, sizeof(info));
    if (n != -1 || errno != EAGAIN) {
        printf("CONTRACT_FAIL empty_eagain: n=%ld errno=%d\n", (long)n, errno);
        return 1;
    }
    printf("empty_eagain: ok\n");

    /* Send SIGUSR1 to self */
    kill(getpid(), SIGUSR1);

    /* Read should return 128-byte signalfd_siginfo */
    n = read(sfd, &info, sizeof(info));
    if (n != (ssize_t)sizeof(info)) {
        printf("CONTRACT_FAIL read_size: n=%ld expected=%lu\n",
               (long)n, (unsigned long)sizeof(info));
        return 1;
    }
    if (info.ssi_signo != SIGUSR1) {
        printf("CONTRACT_FAIL ssi_signo: got=%u expected=%d\n", info.ssi_signo, SIGUSR1);
        return 1;
    }
    printf("ssi_signo: ok (%u)\n", info.ssi_signo);

    /* No more pending → EAGAIN */
    errno = 0;
    n = read(sfd, &info, sizeof(info));
    if (n != -1 || errno != EAGAIN) {
        printf("CONTRACT_FAIL post_eagain: n=%ld errno=%d\n", (long)n, errno);
        return 1;
    }
    printf("post_eagain: ok\n");

    close(sfd);
    printf("CONTRACT_PASS\n");
    return 0;
}
