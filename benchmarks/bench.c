/*
 * SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
 *
 * Kevlar kernel micro-benchmark suite.
 * Compiled as a static musl binary and included in the initramfs.
 *
 * Usage: /bin/bench [--quick|-q] [--full|-f] [--extended|-e] [test-name|all|core|extended]
 *   all          — run all benchmarks (default)
 *   core         — run core 8 benchmarks only
 *   extended     — run extended 16 benchmarks only
 *   <name>       — run a single named benchmark
 *
 * Output format: BENCH <name> <iterations> <total_ns> <per_iter_ns>
 * This format is parsed by the benchmark runner.
 */
#define _GNU_SOURCE
#include <sys/sysmacros.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/uio.h>
#include <sys/resource.h>
#include <sys/utsname.h>
#include <sys/wait.h>
#include <sched.h>
#include <time.h>
#include <unistd.h>
#include <linux/reboot.h>

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

/* ── Init mode setup (when running as PID 1 in a VM) ────────────────── */

static void init_setup(void) {
    if (getpid() != 1) return;

    mkdir("/proc", 0755);
    mkdir("/sys", 0755);
    mkdir("/dev", 0755);
    mkdir("/tmp", 0755);

    /* These may fail on Kevlar (ENOSYS) — that's fine. */
    mount("proc", "/proc", "proc", 0, NULL);
    mount("sysfs", "/sys", "sysfs", 0, NULL);
    mount("devtmpfs", "/dev", "devtmpfs", 0, NULL);
    mount("tmpfs", "/tmp", "tmpfs", 0, NULL);

    /* Ensure /dev/null exists */
    if (open("/dev/null", O_RDONLY) < 0)
        mknod("/dev/null", S_IFCHR | 0666, makedev(1, 3));
}

/* ── Core benchmarks (8) ────────────────────────────────────────────── */

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
    if (fd < 0) { printf("BENCH_SKIP read_null\n"); return; }
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
    if (fd < 0) { printf("BENCH_SKIP write_null\n"); return; }
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
    if (pipe(fds) < 0) { printf("BENCH_SKIP pipe\n"); return; }

    char buf[4096];
    memset(buf, 'A', sizeof(buf));
    int chunks = ITERS(256, 32);

    long long start = now_ns();
    for (int i = 0; i < chunks; i++) {
        write(fds[1], buf, sizeof(buf));
        read(fds[0], buf, sizeof(buf));
    }
    long long elapsed = now_ns() - start;
    report("pipe", chunks, elapsed);

    double secs = elapsed / 1e9;
    double mbps = (chunks * 4096.0 / (1024*1024)) / secs;
    printf("BENCH_EXTRA pipe_throughput_MBps %.1f\n", mbps);

    close(fds[0]);
    close(fds[1]);
}

static void bench_fork(void) {
    int iters = ITERS(200, 50);
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
            printf("BENCH_SKIP fork_exit (fork failed at iter %d)\n", i);
            break;
        }
    }
    if (completed > 0)
        report("fork_exit", completed, now_ns() - start);
}

static void bench_open_close(void) {
    int fd = open("/tmp/benchfile", O_CREAT | O_WRONLY, 0644);
    if (fd < 0) { printf("BENCH_SKIP open_close\n"); return; }
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
    if (p == MAP_FAILED) { printf("BENCH_SKIP mmap_fault\n"); return; }
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

/* ── Extended benchmarks (16) ───────────────────────────────────────── */

/* Category: Syscall round-trip */

static void bench_gettid(void) {
    int iters = ITERS(1000000, 10000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        syscall(SYS_gettid);
    }
    report("gettid", iters, now_ns() - start);
}

static void bench_clock_gettime(void) {
    struct timespec ts;
    int iters = ITERS(1000000, 10000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        clock_gettime(CLOCK_MONOTONIC, &ts);
    }
    report("clock_gettime", iters, now_ns() - start);
}

static void bench_uname(void) {
    struct utsname u;
    int iters = ITERS(500000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        uname(&u);
    }
    report("uname", iters, now_ns() - start);
}

/* Category: FD operations */

static void bench_dup_close(void) {
    int fd = open("/dev/null", O_RDONLY);
    if (fd < 0) { printf("BENCH_SKIP dup_close\n"); return; }
    int iters = ITERS(500000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        int nfd = dup(fd);
        close(nfd);
    }
    report("dup_close", iters, now_ns() - start);
    close(fd);
}

static void bench_fcntl_getfl(void) {
    int fd = open("/dev/null", O_RDONLY);
    if (fd < 0) { printf("BENCH_SKIP fcntl_getfl\n"); return; }
    int iters = ITERS(1000000, 10000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        fcntl(fd, F_GETFL);
    }
    report("fcntl_getfl", iters, now_ns() - start);
    close(fd);
}

static void bench_fstat(void) {
    int fd = open("/dev/null", O_RDONLY);
    if (fd < 0) { printf("BENCH_SKIP fstat\n"); return; }
    struct stat st;
    int iters = ITERS(500000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        fstat(fd, &st);
    }
    report("fstat", iters, now_ns() - start);
    close(fd);
}

static void bench_lseek(void) {
    int fd = open("/dev/null", O_RDONLY);
    if (fd < 0) { printf("BENCH_SKIP lseek\n"); return; }
    int iters = ITERS(1000000, 10000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        lseek(fd, 0, SEEK_CUR);
    }
    report("lseek", iters, now_ns() - start);
    close(fd);
}

/* Category: Path resolution */

static void bench_getcwd(void) {
    char buf[256];
    int iters = ITERS(500000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        getcwd(buf, sizeof(buf));
    }
    report("getcwd", iters, now_ns() - start);
}

static void bench_readlink(void) {
    /* Create a symlink to benchmark readlink */
    int fd = open("/tmp/readlink_target", O_CREAT | O_WRONLY, 0644);
    if (fd < 0) { printf("BENCH_SKIP readlink\n"); return; }
    close(fd);
    unlink("/tmp/readlink_sym");
    if (symlink("/tmp/readlink_target", "/tmp/readlink_sym") < 0) {
        printf("BENCH_SKIP readlink (symlink failed)\n");
        unlink("/tmp/readlink_target");
        return;
    }

    char buf[256];
    int iters = ITERS(200000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        readlink("/tmp/readlink_sym", buf, sizeof(buf));
    }
    report("readlink", iters, now_ns() - start);

    unlink("/tmp/readlink_sym");
    unlink("/tmp/readlink_target");
}

static void bench_access(void) {
    int iters = ITERS(200000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        access("/tmp", F_OK);
    }
    report("access", iters, now_ns() - start);
}

/* Category: Memory management */

static void bench_mmap_munmap(void) {
    int iters = ITERS(100000, 2000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        void *p = mmap(NULL, 4096, PROT_READ | PROT_WRITE,
                       MAP_ANONYMOUS | MAP_PRIVATE, -1, 0);
        if (p != MAP_FAILED)
            munmap(p, 4096);
    }
    report("mmap_munmap", iters, now_ns() - start);
}

static void bench_mprotect(void) {
    void *p = mmap(NULL, 4096, PROT_READ | PROT_WRITE,
                   MAP_ANONYMOUS | MAP_PRIVATE, -1, 0);
    if (p == MAP_FAILED) { printf("BENCH_SKIP mprotect\n"); return; }
    /* Touch to allocate */
    *(volatile char *)p = 0;
    int iters = ITERS(200000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        mprotect(p, 4096, PROT_READ);
        mprotect(p, 4096, PROT_READ | PROT_WRITE);
    }
    report("mprotect", iters, now_ns() - start);
    munmap(p, 4096);
}

static void bench_brk(void) {
    void *base = sbrk(0);
    int iters = ITERS(500000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        brk(base + 4096);
        brk(base);
    }
    report("brk", iters, now_ns() - start);
}

/* Category: Signals */

static void bench_sigaction_noop(void) {
    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = SIG_IGN;
    int iters = ITERS(500000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        sigaction(SIGUSR1, &sa, NULL);
    }
    report("sigaction", iters, now_ns() - start);
}

static void bench_sigprocmask(void) {
    sigset_t set, old;
    sigemptyset(&set);
    sigaddset(&set, SIGUSR1);
    int iters = ITERS(500000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        sigprocmask(SIG_BLOCK, &set, &old);
        sigprocmask(SIG_UNBLOCK, &set, NULL);
    }
    report("sigprocmask", iters, now_ns() - start);
}

/* Category: I/O patterns */

static void bench_pread(void) {
    int fd = open("/dev/null", O_RDONLY);
    if (fd < 0) { printf("BENCH_SKIP pread\n"); return; }
    char buf[1];
    int iters = ITERS(500000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        pread(fd, buf, 1, 0);
    }
    report("pread", iters, now_ns() - start);
    close(fd);
}

static void bench_writev(void) {
    int fd = open("/dev/null", O_WRONLY);
    if (fd < 0) { printf("BENCH_SKIP writev\n"); return; }
    char buf[1] = {0};
    struct iovec iov = { .iov_base = buf, .iov_len = 1 };
    int iters = ITERS(500000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        writev(fd, &iov, 1);
    }
    report("writev", iters, now_ns() - start);
    close(fd);
}

/* ── M6.6 benchmarks (4) ───────────────────────────────────────────── */

static void bench_sched_yield(void) {
    int iters = ITERS(500000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        sched_yield();
    }
    report("sched_yield", iters, now_ns() - start);
}

static void bench_getpriority(void) {
    int iters = ITERS(500000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        getpriority(PRIO_PROCESS, 0);
    }
    report("getpriority", iters, now_ns() - start);
}

static void bench_read_zero(void) {
    int fd = open("/dev/zero", O_RDONLY);
    if (fd < 0) { printf("BENCH_SKIP read_zero\n"); return; }
    char buf[4096];
    int iters = ITERS(200000, 2000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        read(fd, buf, sizeof(buf));
    }
    report("read_zero", iters, now_ns() - start);
    close(fd);
}

static volatile int bench_sig_count;
static void bench_sig_handler(int sig) { (void)sig; bench_sig_count++; }

static void bench_signal_delivery(void) {
    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = bench_sig_handler;
    sigaction(SIGUSR1, &sa, NULL);
    bench_sig_count = 0;
    int iters = ITERS(200000, 2000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        raise(SIGUSR1);
    }
    report("signal_delivery", iters, now_ns() - start);
    /* Restore default */
    sa.sa_handler = SIG_DFL;
    sigaction(SIGUSR1, &sa, NULL);
}

/* ── Benchmark registry ─────────────────────────────────────────────── */

typedef struct {
    const char *name;
    void (*fn)(void);
    int is_core;  /* 1 = core benchmark, 0 = extended */
} bench_entry;

static bench_entry benchmarks[] = {
    /* Core benchmarks (8) */
    {"getpid",       bench_getpid,       1},
    {"read_null",    bench_read_null,    1},
    {"write_null",   bench_write_null,   1},
    {"pipe",         bench_pipe,         1},
    {"fork_exit",    bench_fork,         1},
    {"open_close",   bench_open_close,   1},
    {"mmap_fault",   bench_mmap_fault,   1},
    {"stat",         bench_stat,         1},

    /* Extended benchmarks (16) */
    {"gettid",       bench_gettid,       0},
    {"clock_gettime",bench_clock_gettime,0},
    {"uname",        bench_uname,        0},
    {"dup_close",    bench_dup_close,    0},
    {"fcntl_getfl",  bench_fcntl_getfl,  0},
    {"fstat",        bench_fstat,        0},
    {"lseek",        bench_lseek,        0},
    {"getcwd",       bench_getcwd,       0},
    {"readlink",     bench_readlink,     0},
    {"access",       bench_access,       0},
    {"mmap_munmap",  bench_mmap_munmap,  0},
    {"mprotect",     bench_mprotect,     0},
    {"brk",          bench_brk,          0},
    {"sigaction",    bench_sigaction_noop,0},
    {"sigprocmask",  bench_sigprocmask,  0},
    {"pread",        bench_pread,        0},
    {"writev",       bench_writev,       0},

    /* M6.6 benchmarks (4) */
    {"sched_yield",  bench_sched_yield,  0},
    {"getpriority",  bench_getpriority,  0},
    {"read_zero",    bench_read_zero,    0},
    {"signal_delivery", bench_signal_delivery, 0},

    {NULL, NULL, 0}
};

static int should_run(bench_entry *b, const char *filter) {
    if (strcmp(filter, "all") == 0) return 1;
    if (strcmp(filter, "core") == 0) return b->is_core;
    if (strcmp(filter, "extended") == 0) return !b->is_core;
    /* Support comma-separated names: "mmap_fault,clock_gettime,sigprocmask" */
    const char *p = filter;
    while (*p) {
        const char *comma = strchr(p, ',');
        size_t len = comma ? (size_t)(comma - p) : strlen(p);
        if (len == strlen(b->name) && strncmp(p, b->name, len) == 0)
            return 1;
        if (!comma) break;
        p = comma + 1;
    }
    return 0;
}

int main(int argc, char **argv) {
    const char *filter = "all";

    /* Auto-enable quick mode when running as init (PID 1) in QEMU */
    if (getpid() == 1) {
        quick_mode = 1;
        init_setup();
    }

    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "--quick") == 0 || strcmp(argv[i], "-q") == 0)
            quick_mode = 1;
        else if (strcmp(argv[i], "--full") == 0 || strcmp(argv[i], "-f") == 0)
            quick_mode = 0;
        else
            filter = argv[i];
    }

    printf("BENCH_START kevlar\n");
    if (quick_mode)
        printf("BENCH_MODE quick\n");
    fflush(stdout);
    for (bench_entry *b = benchmarks; b->name; b++) {
        if (should_run(b, filter)) {
            b->fn();
            fflush(stdout);
        }
    }
    printf("BENCH_END\n");
    fflush(stdout);

    /* If running as init (PID 1), power off the system */
    if (getpid() == 1) {
        sync();
        syscall(SYS_reboot, LINUX_REBOOT_MAGIC1, LINUX_REBOOT_MAGIC2,
                LINUX_REBOOT_CMD_POWER_OFF, NULL);
    }

    return 0;
}
