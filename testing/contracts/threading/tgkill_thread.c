/* Contract: tgkill delivers signal to specific thread;
 * wrong tgid → error. Uses pthreads for thread creation. */
#define _GNU_SOURCE
#include <errno.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <unistd.h>

static volatile int sig_received = 0;
static volatile pid_t thread_tid = 0;

static void usr1_handler(int sig) {
    (void)sig;
    sig_received = 1;
}

static void *thread_fn(void *arg) {
    (void)arg;
    thread_tid = syscall(SYS_gettid);
    /* Wait for signal or timeout */
    int tries = 0;
    while (!sig_received && tries < 200) {
        usleep(1000);
        tries++;
    }
    return NULL;
}

int main(void) {
    struct sigaction sa = {0};
    sa.sa_handler = usr1_handler;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGUSR1, &sa, NULL);

    pthread_t thr;
    pthread_create(&thr, NULL, thread_fn, NULL);

    /* Wait for thread to report its tid */
    while (thread_tid == 0) usleep(1000);
    printf("thread_tid: ok\n");

    /* tgkill with correct tgid */
    pid_t tgid = getpid();
    int r = syscall(SYS_tgkill, tgid, thread_tid, SIGUSR1);
    if (r != 0) {
        printf("CONTRACT_FAIL tgkill: ret=%d errno=%d\n", r, errno);
        return 1;
    }

    /* Wait for handler to run */
    int tries = 0;
    while (!sig_received && tries < 100) {
        usleep(1000);
        tries++;
    }
    if (!sig_received) {
        printf("CONTRACT_FAIL signal_delivered: sig_received=%d\n", sig_received);
        return 1;
    }
    printf("tgkill: ok\n");

    /* Wrong tgid → error */
    errno = 0;
    r = syscall(SYS_tgkill, 999999, thread_tid, SIGUSR1);
    if (r != -1) {
        printf("CONTRACT_FAIL wrong_tgid: ret=%d\n", r);
        return 1;
    }
    printf("wrong_tgid: ok\n");

    pthread_join(thr, NULL);
    printf("CONTRACT_PASS\n");
    return 0;
}
