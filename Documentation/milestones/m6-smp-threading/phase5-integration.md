# Phase 5: Integration Testing

**Goal:** Verify that SMP boot, per-CPU scheduling, threading, and thread
safety all work together. Run a real pthreads program linked against musl
on 4 CPUs.

## Test Program: `mini_threads.c`

A static musl binary that exercises all M6 functionality:

```c
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

static atomic_int shared_counter = 0;
static atomic_int barrier_count = 0;
static int NUM_THREADS = 4;

#define TEST(name) static int test_##name(void)
#define RUN(name) do { \
    printf("TEST %-30s ", #name); \
    if (test_##name()) printf("OK\n"); \
    else { printf("FAIL\n"); failures++; } \
} while(0)

/* 1. Basic thread creation and join */
TEST(thread_create_join) {
    pthread_t t;
    int result = 0;
    void *thread_fn(void *arg) {
        *(int *)arg = 42;
        return (void *)123;
    }
    if (pthread_create(&t, NULL, thread_fn, &result)) return 0;
    void *retval;
    if (pthread_join(t, &retval)) return 0;
    return result == 42 && retval == (void *)123;
}

/* 2. gettid returns different values per thread */
TEST(gettid_unique) {
    pid_t parent_tid = syscall(SYS_gettid);
    pid_t child_tid = 0;
    pthread_t t;
    void *fn(void *arg) {
        *(pid_t *)arg = syscall(SYS_gettid);
        return NULL;
    }
    pthread_create(&t, NULL, fn, &child_tid);
    pthread_join(t, NULL);
    return parent_tid != child_tid && child_tid != 0;
}

/* 3. getpid returns same TGID for all threads */
TEST(getpid_same) {
    pid_t parent_pid = getpid();
    pid_t child_pid = 0;
    pthread_t t;
    void *fn(void *arg) {
        *(pid_t *)arg = getpid();
        return NULL;
    }
    pthread_create(&t, NULL, fn, &child_pid);
    pthread_join(t, NULL);
    return parent_pid == child_pid;
}

/* 4. Shared memory between threads */
TEST(shared_memory) {
    volatile int shared = 0;
    pthread_t t;
    void *fn(void *arg) {
        *(volatile int *)arg = 99;
        return NULL;
    }
    pthread_create(&t, NULL, fn, (void *)&shared);
    pthread_join(t, NULL);
    return shared == 99;
}

/* 5. Parallel atomic counter increment */
TEST(atomic_counter) {
    atomic_store(&shared_counter, 0);
    pthread_t threads[4];
    void *fn(void *arg) {
        for (int i = 0; i < 1000; i++)
            atomic_fetch_add(&shared_counter, 1);
        return NULL;
    }
    for (int i = 0; i < 4; i++)
        pthread_create(&threads[i], NULL, fn, NULL);
    for (int i = 0; i < 4; i++)
        pthread_join(threads[i], NULL);
    return atomic_load(&shared_counter) == 4000;
}

/* 6. Mutex correctness */
TEST(mutex) {
    int count = 0;
    pthread_mutex_t mtx = PTHREAD_MUTEX_INITIALIZER;
    pthread_t threads[4];
    struct { int *count; pthread_mutex_t *mtx; } ctx = { &count, &mtx };
    void *fn(void *arg) {
        typeof(ctx) *c = arg;
        for (int i = 0; i < 1000; i++) {
            pthread_mutex_lock(c->mtx);
            (*c->count)++;
            pthread_mutex_unlock(c->mtx);
        }
        return NULL;
    }
    for (int i = 0; i < 4; i++)
        pthread_create(&threads[i], NULL, fn, &ctx);
    for (int i = 0; i < 4; i++)
        pthread_join(threads[i], NULL);
    pthread_mutex_destroy(&mtx);
    return count == 4000;
}

/* 7. Thread-local storage (TLS) */
TEST(tls) {
    static __thread int tls_var = 0;
    pthread_t t;
    void *fn(void *arg) {
        tls_var = 77;
        *(int *)arg = tls_var;
        return NULL;
    }
    tls_var = 11;
    int child_val = 0;
    pthread_create(&t, NULL, fn, &child_val);
    pthread_join(t, NULL);
    return tls_var == 11 && child_val == 77;
}

/* 8. Condition variable */
TEST(condvar) {
    pthread_mutex_t mtx = PTHREAD_MUTEX_INITIALIZER;
    pthread_cond_t cond = PTHREAD_COND_INITIALIZER;
    int ready = 0;
    struct { pthread_mutex_t *m; pthread_cond_t *c; int *r; } ctx = { &mtx, &cond, &ready };
    pthread_t t;
    void *fn(void *arg) {
        typeof(ctx) *c = arg;
        pthread_mutex_lock(c->m);
        *c->r = 1;
        pthread_cond_signal(c->c);
        pthread_mutex_unlock(c->m);
        return NULL;
    }
    pthread_mutex_lock(&mtx);
    pthread_create(&t, NULL, fn, &ctx);
    while (!ready)
        pthread_cond_wait(&cond, &mtx);
    pthread_mutex_unlock(&mtx);
    pthread_join(t, NULL);
    pthread_mutex_destroy(&mtx);
    pthread_cond_destroy(&cond);
    return ready == 1;
}

/* 9. Signal delivery to thread group */
TEST(signal_group) {
    static volatile sig_atomic_t got_signal = 0;
    void handler(int sig) { got_signal = 1; }
    signal(SIGUSR1, handler);
    kill(getpid(), SIGUSR1);
    return got_signal == 1;
}

/* 10. tgkill sends to specific thread */
TEST(tgkill) {
    static volatile sig_atomic_t handler_tid = 0;
    void handler(int sig) { handler_tid = syscall(SYS_gettid); }
    signal(SIGUSR2, handler);
    pid_t target_tid = 0;
    pthread_t t;
    void *fn(void *arg) {
        *(pid_t *)arg = syscall(SYS_gettid);
        usleep(100000); /* 100ms — wait for signal */
        return NULL;
    }
    pthread_create(&t, NULL, fn, &target_tid);
    usleep(10000); /* 10ms — let thread start */
    syscall(SYS_tgkill, getpid(), target_tid, SIGUSR2);
    pthread_join(t, NULL);
    return handler_tid == target_tid;
}

/* 11. mmap/munmap visible across threads */
TEST(mmap_shared) {
    void *addr = mmap(NULL, 4096, PROT_READ|PROT_WRITE,
                      MAP_PRIVATE|MAP_ANONYMOUS, -1, 0);
    if (addr == MAP_FAILED) return 0;
    *(int *)addr = 0;
    pthread_t t;
    void *fn(void *arg) {
        *(int *)arg = 12345;
        return NULL;
    }
    pthread_create(&t, NULL, fn, addr);
    pthread_join(t, NULL);
    int val = *(int *)addr;
    munmap(addr, 4096);
    return val == 12345;
}

/* 12. Fork from threaded process */
TEST(fork_from_thread) {
    pid_t pid = fork();
    if (pid < 0) return 0;
    if (pid == 0) {
        /* Child: only calling thread survives fork */
        _exit(getpid() != 0 ? 0 : 1);
    }
    int status;
    waitpid(pid, &status, 0);
    return WIFEXITED(status) && WEXITSTATUS(status) == 0;
}

int main(void) {
    int failures = 0;
    printf("\n=== Kevlar M6 SMP & Threading Tests ===\n");
    printf("PID: %d, CPUs: %ld\n\n", getpid(), sysconf(_SC_NPROCESSORS_ONLN));

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

    printf("\n%d/%d tests passed\n", 12 - failures, 12);
    return failures ? 1 : 0;
}
```

## Build

```makefile
# In testing/Dockerfile
FROM alpine:3.19 AS mini_threads
RUN apk add --no-cache gcc musl-dev
COPY testing/mini_threads.c /tmp/
RUN gcc -static -O2 -pthread -o /tmp/mini_threads /tmp/mini_threads.c
```

## QEMU Invocation

```bash
# Test with 4 CPUs
qemu-system-x86_64 -smp 4 -m 256M -kernel kernel.elf \
    -initrd initramfs.cpio.gz -nographic \
    -append "init=/bin/mini_threads"

# ARM64
qemu-system-aarch64 -machine virt -cpu cortex-a72 -smp 4 -m 256M \
    -kernel kernel.elf -initrd initramfs.cpio.gz -nographic \
    -append "init=/bin/mini_threads"
```

## Regression Tests

All existing tests must also pass with `-smp 4`:

- BusyBox shell interactive usage
- `mini_systemd` (M4 integration test)
- `bench` (M3 performance benchmarks)
- Fork bomb (limited) — no deadlocks or panics

## Stress Tests

Beyond the functional tests, run stress scenarios:

1. **Thread storm:** 16 threads on 4 CPUs, each doing tight atomic increments
2. **Fork + thread:** Parent creates 4 threads, each thread forks a child
3. **Signal storm:** 4 threads sending signals to each other via tgkill
4. **mmap contention:** 4 threads doing mmap/munmap in parallel
5. **Pipe ping-pong across CPUs:** Two threads on different CPUs passing data
   through a pipe

## Success Criteria

- All 12 `mini_threads` tests pass on x86_64 with `-smp 4`
- All 12 tests pass on ARM64 with `-smp 4`
- All existing M3/M4 tests pass with `-smp 4` (regression)
- No kernel panics, deadlocks, or data corruption under stress
- BusyBox shell responsive on all 4 CPUs simultaneously
