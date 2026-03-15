/* Fork microbenchmarks to isolate overhead sources. */
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <sys/mount.h>
#include <time.h>
#include <sched.h>

static long long now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (long long)ts.tv_sec * 1000000000LL + ts.tv_nsec;
}

static void report(const char *name, int n, long long ns) {
    printf("FORK_MICRO %-20s %4d iters  %7lld ns/iter\n", name, n, ns/n);
}

int main(void) {
    if (getpid() == 1) {
        mkdir("/proc", 0755);
        mount("proc", "/proc", "proc", 0, NULL);
    }

    /* Warm up */
    for (int i = 0; i < 10; i++) {
        pid_t p = fork();
        if (p == 0) _exit(0);
        waitpid(p, NULL, 0);
    }

    /* Test 1: vfork + _exit + wait */
    {
        int N = 200;
        long long t = now_ns();
        for (int i = 0; i < N; i++) {
            pid_t p = vfork();
            if (p == 0) _exit(0);
            waitpid(p, NULL, 0);
        }
        report("vfork+_exit+wait", N, now_ns() - t);
    }

    /* Test 2: vfork phase detail */
    {
        int N = 200;
        long long total_vfork = 0, total_wait = 0;
        for (int i = 0; i < N; i++) {
            long long t0 = now_ns();
            pid_t p = vfork();
            if (p == 0) _exit(0);
            long long t1 = now_ns();
            waitpid(p, NULL, 0);
            long long t2 = now_ns();
            total_vfork += (t1 - t0);
            total_wait += (t2 - t1);
        }
        printf("VFORK_DETAIL vfork_call=%lld waitpid=%lld total=%lld (ns/iter)\n",
               total_vfork/N, total_wait/N, (total_vfork+total_wait)/N);
    }

    /* Test 3: context switch round trip */
    {
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

    /* Test 4: fork+exit without waitpid */
    {
        int N = 200;
        long long t = now_ns();
        for (int i = 0; i < N; i++) {
            pid_t p = fork();
            if (p == 0) _exit(0);
        }
        long long elapsed = now_ns() - t;
        while (waitpid(-1, NULL, WNOHANG) > 0);
        report("fork+exit(nowait)", N, elapsed);
    }

    /* Test 5: waitpid on already-exited child */
    {
        pid_t pids[200];
        for (int i = 0; i < 200; i++) {
            pids[i] = fork();
            if (pids[i] == 0) _exit(0);
        }
        usleep(10000);
        int N = 200;
        long long t = now_ns();
        for (int i = 0; i < N; i++) {
            waitpid(pids[i], NULL, 0);
        }
        report("waitpid_only", N, now_ns() - t);
    }

    /* Test 6: first vs warm context switch to forked child */
    {
        int p1[2], p2[2];
        pipe(p1); pipe(p2);
        pid_t p = fork();
        if (p == 0) {
            char c;
            for (int i = 0; i < 11; i++) {
                read(p1[0], &c, 1);
                write(p2[1], &c, 1);
            }
            _exit(0);
        }
        char c = 'x';
        /* First round trip: cold child */
        long long t0 = now_ns();
        write(p1[1], &c, 1);
        read(p2[0], &c, 1);
        long long t1 = now_ns();
        /* 10 more warm round trips */
        for (int i = 0; i < 10; i++) {
            write(p1[1], &c, 1);
            read(p2[0], &c, 1);
        }
        long long t2 = now_ns();
        close(p1[0]); close(p1[1]); close(p2[0]); close(p2[1]);
        waitpid(p, NULL, 0);
        printf("FIRST_SWITCH first=%lldns warm_avg=%lldns\n",
               t1 - t0, (t2 - t1) / 10);
    }

    /* Test 7: fork + ONE pipe round trip + exit (cold child switch cost) */
    {
        int N = 200;
        long long t = now_ns();
        for (int i = 0; i < N; i++) {
            int pp[2];
            pipe(pp);
            pid_t p = fork();
            if (p == 0) {
                char c;
                read(pp[0], &c, 1); /* wait for parent */
                _exit(0);
            }
            char c = 'x';
            write(pp[1], &c, 1); /* wake child */
            waitpid(p, NULL, 0);
            close(pp[0]); close(pp[1]);
        }
        report("fork+1pipe+exit", N, now_ns() - t);
    }

    /* Test 7: full fork+exit+wait */
    {
        int N = 200;
        long long t = now_ns();
        for (int i = 0; i < N; i++) {
            pid_t p = fork();
            if (p == 0) _exit(0);
            waitpid(p, NULL, 0);
        }
        report("fork+_exit+wait", N, now_ns() - t);
    }

    if (getpid() == 1) {
        syscall(SYS_reboot, 0xfee1dead, 0x28121969, 0x4321fedc, 0);
    }
    return 0;
}
