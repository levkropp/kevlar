// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// M6 Phase 3 integration test: pthreads on Kevlar.
//
// Build: musl-gcc -static -O2 -pthread -o mini-threads testing/mini_threads.c
// Run:   /bin/mini-threads
//
// Tests clone(CLONE_VM|CLONE_THREAD), futex wait/wake, TLS, signals,
// tgkill, and mmap visibility across threads.
//
// Output format (grep-able by test runner):
//   TEST_PASS <name>
//   TEST_FAIL <name>
//   TEST_END  <passed>/<total>
//
#define _GNU_SOURCE
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <signal.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <sys/mman.h>
#include <stdatomic.h>
#include <errno.h>

static int g_failures = 0;
static int g_total    = 0;

#define RUN(name) do {                                                  \
    g_total++;                                                          \
    int _ok = test_##name();                                            \
    if (_ok) { printf("TEST_PASS %-30s\n", #name); fflush(stdout); }   \
    else     { printf("TEST_FAIL %-30s\n", #name); fflush(stdout); g_failures++; } \
} while (0)

/* ─── 1. Basic thread create + join ─────────────────────────────────────── */
static void *t1_fn(void *arg) { *(int *)arg = 42; return (void *)123; }
static int test_thread_create_join(void) {
    pthread_t t; int val = 0; void *ret;
    if (pthread_create(&t, NULL, t1_fn, &val)) return 0;
    if (pthread_join(t, &ret)) return 0;
    return val == 42 && ret == (void *)123;
}

/* ─── 2. gettid returns different values ─────────────────────────────────── */
static void *t2_fn(void *arg) {
    *(pid_t *)arg = (pid_t)syscall(SYS_gettid); return NULL;
}
static int test_gettid_unique(void) {
    pid_t ptid = (pid_t)syscall(SYS_gettid), ctid = 0; pthread_t t;
    pthread_create(&t, NULL, t2_fn, &ctid);
    pthread_join(t, NULL);
    return ptid != ctid && ctid != 0;
}

/* ─── 3. getpid returns same TGID for all threads ────────────────────────── */
static void *t3_fn(void *arg) { *(pid_t *)arg = getpid(); return NULL; }
static int test_getpid_same(void) {
    pid_t ppid = getpid(), cpid = 0; pthread_t t;
    pthread_create(&t, NULL, t3_fn, &cpid);
    pthread_join(t, NULL);
    return ppid == cpid && cpid != 0;
}

/* ─── 4. Shared memory visible across threads ────────────────────────────── */
static void *t4_fn(void *arg) { *(volatile int *)arg = 99; return NULL; }
static int test_shared_memory(void) {
    volatile int shared = 0; pthread_t t;
    pthread_create(&t, NULL, t4_fn, (void *)&shared);
    pthread_join(t, NULL);
    return shared == 99;
}

/* ─── 5. Parallel atomic counter ─────────────────────────────────────────── */
static atomic_int g_counter;
static void *t5_fn(void *arg) {
    (void)arg;
    for (int i = 0; i < 1000; i++) atomic_fetch_add(&g_counter, 1);
    return NULL;
}
static int test_atomic_counter(void) {
    atomic_store(&g_counter, 0);
    pthread_t ts[4];
    for (int i = 0; i < 4; i++) pthread_create(&ts[i], NULL, t5_fn, NULL);
    for (int i = 0; i < 4; i++) pthread_join(ts[i], NULL);
    return atomic_load(&g_counter) == 4000;
}

/* ─── 6. Mutex correctness ────────────────────────────────────────────────── */
struct t6_ctx { int *count; pthread_mutex_t *mtx; };
static void *t6_fn(void *arg) {
    struct t6_ctx *c = arg;
    for (int i = 0; i < 1000; i++) {
        pthread_mutex_lock(c->mtx);
        (*c->count)++;
        pthread_mutex_unlock(c->mtx);
    }
    return NULL;
}
static int test_mutex(void) {
    int count = 0; pthread_mutex_t mtx = PTHREAD_MUTEX_INITIALIZER;
    struct t6_ctx ctx = { &count, &mtx };
    pthread_t ts[4];
    for (int i = 0; i < 4; i++) pthread_create(&ts[i], NULL, t6_fn, &ctx);
    for (int i = 0; i < 4; i++) pthread_join(ts[i], NULL);
    pthread_mutex_destroy(&mtx);
    return count == 4000;
}

/* ─── 7. Thread-local storage ─────────────────────────────────────────────── */
static __thread int tls_var = 0;
static void *t7_fn(void *arg) { tls_var = 77; *(int *)arg = tls_var; return NULL; }
static int test_tls(void) {
    tls_var = 11; int child_val = 0; pthread_t t;
    pthread_create(&t, NULL, t7_fn, &child_val);
    pthread_join(t, NULL);
    return tls_var == 11 && child_val == 77;
}

/* ─── 8. Condition variable ───────────────────────────────────────────────── */
struct t8_ctx { pthread_mutex_t *m; pthread_cond_t *c; int *ready; };
static void *t8_fn(void *arg) {
    struct t8_ctx *ctx = arg;
    pthread_mutex_lock(ctx->m);
    *ctx->ready = 1;
    pthread_cond_signal(ctx->c);
    pthread_mutex_unlock(ctx->m);
    return NULL;
}
static int test_condvar(void) {
    pthread_mutex_t mtx = PTHREAD_MUTEX_INITIALIZER;
    pthread_cond_t  cnd = PTHREAD_COND_INITIALIZER;
    int ready = 0;
    struct t8_ctx ctx = { &mtx, &cnd, &ready };
    pthread_t t;
    pthread_mutex_lock(&mtx);
    pthread_create(&t, NULL, t8_fn, &ctx);
    while (!ready) pthread_cond_wait(&cnd, &mtx);
    pthread_mutex_unlock(&mtx);
    pthread_join(t, NULL);
    pthread_mutex_destroy(&mtx);
    pthread_cond_destroy(&cnd);
    return ready == 1;
}

/* ─── 9. Signal delivered to thread group ────────────────────────────────── */
static volatile sig_atomic_t g_sig9 = 0;
static void sig9_handler(int s) { (void)s; g_sig9 = 1; }
static int test_signal_group(void) {
    signal(SIGUSR1, sig9_handler);
    kill(getpid(), SIGUSR1);
    // Give signal a moment to be delivered.
    for (int i = 0; i < 100 && !g_sig9; i++) sched_yield();
    signal(SIGUSR1, SIG_DFL);
    return g_sig9 == 1;
}

/* ─── 10. tgkill sends signal to specific thread ─────────────────────────── */
static volatile sig_atomic_t g_sig10_tid = 0;
static void sig10_handler(int s) {
    (void)s; g_sig10_tid = (sig_atomic_t)syscall(SYS_gettid);
}
struct t10_ctx { pid_t *tid_out; };
static void *t10_fn(void *arg) {
    struct t10_ctx *ctx = arg;
    *ctx->tid_out = (pid_t)syscall(SYS_gettid);
    // Spin briefly so the main thread has time to tgkill us.
    for (volatile int i = 0; i < 10000000; i++);
    return NULL;
}
static int test_tgkill(void) {
    signal(SIGUSR2, sig10_handler);
    pid_t target_tid = 0;
    struct t10_ctx ctx = { &target_tid };
    pthread_t t;
    pthread_create(&t, NULL, t10_fn, &ctx);
    // Wait until the thread has stored its tid.
    while (target_tid == 0) sched_yield();
    syscall(SYS_tgkill, getpid(), target_tid, SIGUSR2);
    pthread_join(t, NULL);
    signal(SIGUSR2, SIG_DFL);
    return g_sig10_tid == (sig_atomic_t)target_tid && target_tid != 0;
}

/* ─── 11. mmap visible across threads ────────────────────────────────────── */
static void *t11_fn(void *arg) { *(int *)arg = 12345; return NULL; }
static int test_mmap_shared(void) {
    void *addr = mmap(NULL, 4096, PROT_READ|PROT_WRITE,
                      MAP_PRIVATE|MAP_ANONYMOUS, -1, 0);
    if (addr == MAP_FAILED) return 0;
    *(int *)addr = 0;
    pthread_t t;
    pthread_create(&t, NULL, t11_fn, addr);
    pthread_join(t, NULL);
    int val = *(int *)addr;
    munmap(addr, 4096);
    return val == 12345;
}

/* ─── 12. Fork from threaded process ─────────────────────────────────────── */
static int test_fork_from_thread(void) {
    pid_t pid = fork();
    if (pid < 0) return 0;
    if (pid == 0) { _exit(0); }
    int status;
    waitpid(pid, &status, 0);
    return WIFEXITED(status) && WEXITSTATUS(status) == 0;
}

/* ─── 13. Pipe ping-pong across threads ─────────────────────────────────── */
// Two threads bounce a counter through two pipes 100 times.
// Tests: blocking read/write, scheduler wakeup from WaitQueue, cross-CPU
// delivery of futex-wake when a thread unblocks another via pipe write.
struct t13_ctx { int ping[2]; int pong[2]; };
static void *t13_fn(void *arg) {
    struct t13_ctx *p = arg;
    for (int i = 0; i < 100; i++) {
        int val;
        if (read(p->ping[0], &val, sizeof(val)) != sizeof(val)) return (void*)1;
        val++;
        if (write(p->pong[1], &val, sizeof(val)) != sizeof(val)) return (void*)1;
    }
    return NULL;
}
static int test_pipe_pingpong(void) {
    struct t13_ctx p;
    if (pipe(p.ping) || pipe(p.pong)) return 0;
    pthread_t t;
    if (pthread_create(&t, NULL, t13_fn, &p)) return 0;
    int val = 0;
    for (int i = 0; i < 100; i++) {
        if (write(p.ping[1], &val, sizeof(val)) != sizeof(val)) return 0;
        if (read(p.pong[0], &val, sizeof(val)) != sizeof(val)) return 0;
    }
    void *rv;
    pthread_join(t, &rv);
    close(p.ping[0]); close(p.ping[1]);
    close(p.pong[0]); close(p.pong[1]);
    return val == 100 && rv == NULL;
}

/* ─── 14. Thread storm: 16 threads on 4 CPUs ────────────────────────────── */
// More threads than CPUs; exercises the scheduler's ability to context-switch
// and distribute work across all vCPUs under load.
static void *t14_fn(void *arg) {
    atomic_int *ctr = arg;
    for (int i = 0; i < 100; i++) atomic_fetch_add(ctr, 1);
    return NULL;
}
static int test_thread_storm(void) {
    atomic_int ctr = 0;
    pthread_t ts[16];
    for (int i = 0; i < 16; i++) pthread_create(&ts[i], NULL, t14_fn, &ctr);
    for (int i = 0; i < 16; i++) pthread_join(ts[i], NULL);
    return atomic_load(&ctr) == 1600;
}

/* ─────────────────────────────────────────────────────────────────────────── */
int main(void) {
    printf("\n=== Kevlar M6 Threading Tests ===\n");
    printf("PID=%d  TID=%d  CPUs=%ld\n\n",
           getpid(),
           (int)syscall(SYS_gettid),
           sysconf(_SC_NPROCESSORS_ONLN));
    fflush(stdout);

    RUN(thread_create_join);
    RUN(gettid_unique);
    RUN(getpid_same);
    RUN(shared_memory);
    RUN(atomic_counter);
    RUN(mutex);
    RUN(tls);
    RUN(condvar);
    RUN(signal_group);
    RUN(tgkill);
    RUN(mmap_shared);
    RUN(fork_from_thread);
    RUN(pipe_pingpong);
    RUN(thread_storm);

    printf("\nTEST_END %d/%d\n", g_total - g_failures, g_total);
    return g_failures ? 1 : 0;
}
