/* Contract: futex WAIT blocks; WAKE unblocks;
 * wrong expected value → EAGAIN. Uses pthreads for thread creation. */
#define _GNU_SOURCE
#include <errno.h>
#include <linux/futex.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <unistd.h>

static volatile uint32_t futex_var = 0;
static volatile int thread_done = 0;

static void *waiter_fn(void *arg) {
    (void)arg;
    while (futex_var == 0) {
        syscall(SYS_futex, &futex_var, FUTEX_WAIT, 0, NULL, NULL, 0);
    }
    thread_done = 1;
    return NULL;
}

int main(void) {
    /* FUTEX_WAIT with wrong expected value → EAGAIN */
    uint32_t val = 10;
    errno = 0;
    int ret = syscall(SYS_futex, &val, FUTEX_WAIT, 99, NULL, NULL, 0);
    if (ret != -1 || errno != EAGAIN) {
        printf("CONTRACT_FAIL eagain: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("eagain: ok\n");

    /* FUTEX_WAKE with no waiters returns 0 */
    ret = syscall(SYS_futex, &val, FUTEX_WAKE, 1, NULL, NULL, 0);
    if (ret != 0) {
        printf("CONTRACT_FAIL wake_empty: ret=%d\n", ret);
        return 1;
    }
    printf("wake_empty: ok\n");

    /* Spawn thread that waits on futex_var */
    pthread_t thr;
    pthread_create(&thr, NULL, waiter_fn, NULL);

    /* Give thread time to enter WAIT */
    usleep(20000);

    /* Set value and wake */
    futex_var = 1;
    ret = syscall(SYS_futex, &futex_var, FUTEX_WAKE, 1, NULL, NULL, 0);
    printf("wake_ret: %d\n", ret);

    pthread_join(thr, NULL);

    if (!thread_done) {
        printf("CONTRACT_FAIL thread_wake: thread_done=%d\n", thread_done);
        return 1;
    }
    printf("thread_woke: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
