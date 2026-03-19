/* Contract: setitimer ITIMER_REAL one-shot fires SIGALRM; cancel returns old value. */
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <sys/time.h>
#include <unistd.h>

static volatile int alarm_fired = 0;

static void handler(int sig) {
    (void)sig;
    alarm_fired = 1;
}

int main(void) {
    struct sigaction sa = {0};
    sa.sa_handler = handler;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGALRM, &sa, NULL);

    /* One-shot 100ms timer */
    struct itimerval itv = {0};
    itv.it_value.tv_usec = 100000; /* 100ms */
    if (setitimer(ITIMER_REAL, &itv, NULL) != 0) {
        printf("CONTRACT_FAIL setitimer: errno=%d\n", errno);
        return 1;
    }

    /* pause() should return with EINTR after SIGALRM */
    errno = 0;
    pause();
    if (errno != EINTR || alarm_fired != 1) {
        printf("CONTRACT_FAIL pause: errno=%d fired=%d\n", errno, alarm_fired);
        return 1;
    }
    printf("oneshot_fire: ok\n");

    /* Set a 10-second timer, then cancel it; old value should show ~10s remaining */
    struct itimerval big = {0};
    big.it_value.tv_sec = 10;
    if (setitimer(ITIMER_REAL, &big, NULL) != 0) {
        printf("CONTRACT_FAIL setitimer_big: errno=%d\n", errno);
        return 1;
    }

    struct itimerval cancel = {0};
    struct itimerval old = {0};
    if (setitimer(ITIMER_REAL, &cancel, &old) != 0) {
        printf("CONTRACT_FAIL cancel: errno=%d\n", errno);
        return 1;
    }
    /* old.it_value should be approximately 10 seconds (9-10) */
    if (old.it_value.tv_sec < 5) {
        printf("CONTRACT_FAIL cancel_remaining: sec=%ld expected>=5\n",
               (long)old.it_value.tv_sec);
        return 1;
    }
    /* Print only seconds — usec varies by CPU speed and is non-deterministic. */
    printf("cancel_remaining: ok sec=%ld\n", (long)old.it_value.tv_sec);

    printf("CONTRACT_PASS\n");
    return 0;
}
