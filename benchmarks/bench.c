/*
 * SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
 *
 * Kevlar kernel micro-benchmark suite.
 * Compiled as a static musl binary and included in the initramfs.
 *
 * Usage: /bin/bench [test-name]
 *   all          — run all benchmarks (default)
 *   getpid       — syscall round-trip latency
 *   read_null    — read(/dev/null) latency
 *   write_null   — write(/dev/null) latency
 *   pipe         — pipe throughput (1 MB)
 *   fork         — fork+exit latency
 *   open_close   — open+close /tmp/benchfile
 *   mmap_fault   — mmap+touch page fault throughput
 *   stat         — stat() latency
 *
 * Output format: BENCH <name> <iterations> <total_ns> <per_iter_ns>
 * This format is parsed by the benchmark runner.
 */
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static long long now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (long long)ts.tv_sec * 1000000000LL + ts.tv_nsec;
}

static void report(const char *name, int iters, long long elapsed_ns) {
    long long per_iter = elapsed_ns / iters;
    printf("BENCH %s %d %lld %lld\n", name, iters, elapsed_ns, per_iter);
}

/* Iteration counts: "quick" mode for QEMU TCG, "full" for native/KVM. */
static int quick_mode = 0;

#define ITERS(full, quick) (quick_mode ? (quick) : (full))

static void bench_getpid(void) {
    int iters = ITERS(1000000, 10000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        getpid();
    }
    report("getpid", iters, now_ns() - start);
}

static void bench_read_null(void) {
    int fd = open("/dev/null", O_RDONLY);
    if (fd < 0) { perror("open /dev/null"); return; }
    char buf[1];
    int iters = ITERS(500000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        read(fd, buf, 1);
    }
    report("read_null", iters, now_ns() - start);
    close(fd);
}

static void bench_write_null(void) {
    int fd = open("/dev/null", O_WRONLY);
    if (fd < 0) { perror("open /dev/null"); return; }
    char buf[1] = {0};
    int iters = ITERS(500000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        write(fd, buf, 1);
    }
    report("write_null", iters, now_ns() - start);
    close(fd);
}

static void bench_pipe(void) {
    int fds[2];
    if (pipe(fds) < 0) { perror("pipe"); return; }

    char buf[4096];
    memset(buf, 'A', sizeof(buf));
    int chunks = ITERS(256, 32); /* 256*4096=1MB, 32*4096=128KB */
    int iters = chunks;

    long long start = now_ns();
    for (int i = 0; i < chunks; i++) {
        write(fds[1], buf, sizeof(buf));
        read(fds[0], buf, sizeof(buf));
    }
    long long elapsed = now_ns() - start;
    report("pipe", iters, elapsed);

    double secs = elapsed / 1e9;
    double mbps = (chunks * 4096.0 / (1024*1024)) / secs;
    printf("BENCH_EXTRA pipe_throughput_MBps %.1f\n", mbps);

    close(fds[0]);
    close(fds[1]);
}

static void bench_fork(void) {
    int iters = ITERS(1000, 50);
    int completed = 0;
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        pid_t pid = fork();
        if (pid == 0) {
            _exit(0);
        } else if (pid > 0) {
            waitpid(pid, NULL, 0);
            completed++;
        } else {
            printf("BENCH_SKIP fork_exit (fork failed: %d)\n", i);
            break;
        }
    }
    if (completed > 0)
        report("fork_exit", completed, now_ns() - start);
}

static void bench_open_close(void) {
    int fd = open("/tmp/benchfile", O_CREAT | O_WRONLY, 0644);
    if (fd < 0) { printf("BENCH_SKIP open_close (create failed)\n"); return; }
    close(fd);

    int iters = ITERS(100000, 2000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        fd = open("/tmp/benchfile", O_RDONLY);
        if (fd >= 0) close(fd);
    }
    report("open_close", iters, now_ns() - start);
    unlink("/tmp/benchfile");
}

static void bench_mmap_fault(void) {
    int pages = ITERS(4096, 256);
    size_t len = (size_t)pages * 4096;
    long long start = now_ns();
    char *p = mmap(NULL, len, PROT_READ | PROT_WRITE,
                   MAP_ANONYMOUS | MAP_PRIVATE, -1, 0);
    if (p == MAP_FAILED) { printf("BENCH_SKIP mmap_fault (mmap failed)\n"); return; }
    for (int i = 0; i < pages; i++) {
        p[i * 4096] = (char)i;
    }
    report("mmap_fault", pages, now_ns() - start);
    munmap(p, len);
}

static void bench_stat(void) {
    struct stat st;
    int iters = ITERS(200000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        stat("/tmp", &st);
    }
    report("stat", iters, now_ns() - start);
}

typedef struct {
    const char *name;
    void (*fn)(void);
} bench_entry;

static bench_entry benchmarks[] = {
    {"getpid",     bench_getpid},
    {"read_null",  bench_read_null},
    {"write_null", bench_write_null},
    {"pipe",       bench_pipe},
    {"fork",       bench_fork},
    {"open_close", bench_open_close},
    {"mmap_fault", bench_mmap_fault},
    {"stat",       bench_stat},
    {NULL, NULL}
};

int main(int argc, char **argv) {
    const char *filter = "all";
    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "--quick") == 0 || strcmp(argv[i], "-q") == 0)
            quick_mode = 1;
        else
            filter = argv[i];
    }

    printf("BENCH_START kevlar\n");
    if (quick_mode)
        printf("BENCH_MODE quick\n");
    fflush(stdout);
    for (bench_entry *b = benchmarks; b->name; b++) {
        if (strcmp(filter, "all") == 0 || strcmp(filter, b->name) == 0) {
            b->fn();
            fflush(stdout);
        }
    }
    printf("BENCH_END\n");
    fflush(stdout);
    return 0;
}
