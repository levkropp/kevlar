/* Contract: pthread creates thread with shared memory;
 * underlying clone sets child_tid; thread exit works correctly.
 * Tests kernel's CLONE_VM|CLONE_THREAD via musl pthreads. */
#define _GNU_SOURCE
#include <errno.h>
#include <pthread.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <unistd.h>

static volatile int shared_val = 0;
static volatile pid_t thread_tid = 0;

static void *thread_fn(void *arg) {
    (void)arg;
    thread_tid = syscall(SYS_gettid);
    shared_val = 42;
    return NULL;
}

int main(void) {
    pid_t main_pid = getpid();
    pid_t main_tid = syscall(SYS_gettid);
    printf("main: ok\n");

    pthread_t thr;
    int ret = pthread_create(&thr, NULL, thread_fn, NULL);
    if (ret != 0) {
        printf("CONTRACT_FAIL pthread_create: ret=%d\n", ret);
        return 1;
    }
    printf("pthread_create: ok\n");

    ret = pthread_join(thr, NULL);
    if (ret != 0) {
        printf("CONTRACT_FAIL pthread_join: ret=%d\n", ret);
        return 1;
    }
    printf("pthread_join: ok\n");

    /* Thread had different tid but same pid (CLONE_THREAD) */
    if (thread_tid == 0) {
        printf("CONTRACT_FAIL thread_tid: not set\n");
        return 1;
    }
    if (thread_tid == main_tid) {
        printf("CONTRACT_FAIL tid_different: both=%d\n", main_tid);
        return 1;
    }
    printf("thread_tid: ok\n");

    /* Shared memory visible (CLONE_VM) */
    if (shared_val != 42) {
        printf("CONTRACT_FAIL shared_mem: val=%d expected=42\n", shared_val);
        return 1;
    }
    printf("shared_mem: ok val=%d\n", shared_val);

    printf("CONTRACT_PASS\n");
    return 0;
}
