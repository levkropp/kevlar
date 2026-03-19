/* Contract: alarm() delivers SIGALRM; alarm(0) cancels. */
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <unistd.h>

static volatile int alarm_fired = 0;

static void alarm_handler(int sig) {
    (void)sig;
    alarm_fired = 1;
}

int main(void) {
    struct sigaction sa = {0};
    sa.sa_handler = alarm_handler;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGALRM, &sa, NULL);

    /* alarm(1) → SIGALRM after ~1 second, pause returns -1/EINTR */
    alarm(1);
    errno = 0;
    pause();
    if (errno != EINTR || alarm_fired != 1) {
        printf("CONTRACT_FAIL alarm_pause: errno=%d fired=%d\n", errno, alarm_fired);
        return 1;
    }
    printf("alarm_pause: ok\n");

    /* alarm(0) cancels and returns remaining seconds */
    alarm_fired = 0;
    unsigned int prev = alarm(10);
    /* prev should be 0 since previous alarm already fired */
    if (prev != 0) {
        printf("alarm_prev: prev=%u (non-zero, previous may have been set)\n", prev);
    }
    unsigned int rem = alarm(0);
    /* rem should be ~10 (was just set) or close */
    if (rem == 0) {
        printf("CONTRACT_FAIL alarm_cancel: rem=%u (expected >0)\n", rem);
        return 1;
    }
    printf("alarm_cancel: ok rem=%u\n", rem);

    printf("CONTRACT_PASS\n");
    return 0;
}
