/* Minimal fork microbenchmarks to isolate overhead sources.
   Uses raw syscalls (no musl cleanup in _exit path).
   Compiled as static binary, runs as PID 1. */
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <time.h>

static long long now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (long long)ts.tv_sec * 1000000000LL + ts.tv_nsec;
}

static void report(const char *name, int n, long long ns) {
    printf("FORK_MICRO %-20s %4d iters  %7lld ns/iter\n", name, n, ns/n);
}

/* Test 1: Just fork + raw exit_group (no musl _exit cleanup) */
static void test_fork_raw_exit(void) {
    int N = 200;
    long long t = now_ns();
    for (int i = 0; i < N; i++) {
        pid_t p = syscall(SYS_fork);
        if (p == 0) syscall(SYS_exit_group, 0);
        syscall(SYS_wait4, p, 0, 0, 0);
    }
    report("fork+raw_exit+wait", N, now_ns() - t);
}

/* Test 2: Just fork + _exit (with musl atexit cleanup) */
static void test_fork_exit(void) {
    int N = 200;
    long long t = now_ns();
    for (int i = 0; i < N; i++) {
        pid_t p = fork();
        if (p == 0) _exit(0);
        waitpid(p, NULL, 0);
    }
    report("fork+_exit+wait", N, now_ns() - t);
}

/* Test 3: Just the fork syscall (no exit, no wait — accumulate zombies) */
static void test_fork_only(void) {
    int N = 200;
    long long t = now_ns();
    for (int i = 0; i < N; i++) {
        pid_t p = syscall(SYS_fork);
        if (p == 0) syscall(SYS_exit_group, 0);
        /* Don't wait — let zombies accumulate */
    }
    long long elapsed = now_ns() - t;
    /* Reap all zombies */
    while (waitpid(-1, NULL, WNOHANG) > 0);
    report("fork+exit(nowait)", N, elapsed);
}

/* Test 4: Just waitpid (pre-fork children that already exited) */
static void test_wait_only(void) {
    /* Pre-create 200 children that exit immediately */
    pid_t pids[200];
    for (int i = 0; i < 200; i++) {
        pids[i] = fork();
        if (pids[i] == 0) _exit(0);
    }
    /* Give children time to exit */
    usleep(10000);
    /* Now time just the waitpid calls */
    int N = 200;
    long long t = now_ns();
    for (int i = 0; i < N; i++) {
        waitpid(pids[i], NULL, 0);
    }
    report("waitpid_only", N, now_ns() - t);
}

/* Test 5: Context switch cost (two processes ping-pong via pipe) */
static void test_ctx_switch(void) {
    int p1[2], p2[2];
    pipe(p1); pipe(p2);
    pid_t pid = fork();
    if (pid == 0) {
        char c;
        for (int i = 0; i < 1000; i++) {
            read(p1[0], &c, 1);
            write(p2[1], &c, 1);
        }
        _exit(0);
    }
    char c = 'x';
    int N = 1000;
    long long t = now_ns();
    for (int i = 0; i < N; i++) {
        write(p1[1], &c, 1);
        read(p2[0], &c, 1);
    }
    report("ctx_switch_rt", N, now_ns() - t);
    close(p1[0]); close(p1[1]); close(p2[0]); close(p2[1]);
    waitpid(pid, NULL, 0);
}

int main(void) {
    /* Run as init: mount /proc for clock_gettime */
    if (getpid() == 1) {
        mkdir("/proc", 0755);
        mount("proc", "/proc", "proc", 0, NULL);
    }

    /* Warm up */
    for (int i = 0; i < 5; i++) {
        pid_t p = fork();
        if (p == 0) _exit(0);
        waitpid(p, NULL, 0);
    }

    test_ctx_switch();
    test_fork_only();
    test_wait_only();
    test_fork_raw_exit();
    test_fork_exit();

    if (getpid() == 1) {
        /* Exit to halt */
        syscall(SYS_reboot, 0xfee1dead, 0x28121969, 0x4321fedc, 0);
    }
    return 0;
}
