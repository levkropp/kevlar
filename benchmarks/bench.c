/*
 * SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
 *
 * Kevlar kernel micro-benchmark suite.
 * Compiled as a static musl binary and included in the initramfs.
 *
 * Usage: /bin/bench [--quick|-q] [--full|-f] [test-name|all|core|extended]
 *   all          — run all benchmarks (default)
 *   core         — run core 8 benchmarks only
 *   extended     — run extended 16 + M6.6 4 + M10 8 + workload 7 benchmarks
 *   <name>       — run a single named benchmark
 *   name1,name2  — run comma-separated list of benchmarks
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
#include <errno.h>
#include <sys/uio.h>
#include <sys/resource.h>
#include <sys/utsname.h>
#include <sys/wait.h>
#include <sys/file.h>
#include <sys/socket.h>
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
    int iters = ITERS(500, 200);
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

/* Kevlar-private SYS_KVLR_VFORK() — vfork-like with ghost-fork CoW.
 * See blog 226.  Same shape as bench_fork but uses kvlr_vfork so the
 * kernel can skip share_leaf_pt's refcount walk + the parent↔child
 * ctx-switch round-trip. */
#ifndef SYS_kvlr_vfork
#define SYS_kvlr_vfork 501
#endif

static void bench_fork_kvlr(void) {
    /* Probe — kvlr_vfork returns -ENOSYS on Linux.  From the child side
     * of the first iter we can't see the errno directly (we just _exit),
     * so probe from the parent side: a first successful call proves
     * support.  ghost-fork semantics mean waitpid on parent unblocks
     * only after child _exit anyway — same shape as fork+wait. */
    long pid = syscall(SYS_kvlr_vfork);
    if (pid < 0) {
        printf("BENCH_SKIP fork_exit_kvlr (kvlr_vfork unsupported)\n");
        return;
    }
    if (pid == 0) {
        _exit(0);
    }
    waitpid((pid_t)pid, NULL, 0);

    int iters = ITERS(500, 200);
    int completed = 0;
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        long p = syscall(SYS_kvlr_vfork);
        if (p == 0) {
            _exit(0);
        } else if (p > 0) {
            waitpid((pid_t)p, NULL, 0);
            completed++;
        } else break;
    }
    if (completed > 0)
        report("fork_exit_kvlr", completed, now_ns() - start);
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
        gettid();
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

/* ── M10 benchmarks (8) ────────────────────────────────────────────── */

#include <sys/epoll.h>
#include <sys/eventfd.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <dirent.h>
#include <poll.h>

static void bench_epoll(void) {
    int epfd = epoll_create1(0);
    if (epfd < 0) { printf("BENCH_SKIP epoll\n"); return; }
    int evfd = eventfd(0, EFD_NONBLOCK);
    struct epoll_event ev = { .events = EPOLLIN, .data.fd = evfd };
    epoll_ctl(epfd, EPOLL_CTL_ADD, evfd, &ev);
    struct epoll_event out;
    int iters = ITERS(500000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        epoll_wait(epfd, &out, 1, 0);
    }
    report("epoll_wait", iters, now_ns() - start);
    close(evfd);
    close(epfd);
}

static void bench_poll_null(void) {
    int fd = open("/dev/null", O_RDONLY);
    if (fd < 0) { printf("BENCH_SKIP poll\n"); return; }
    struct pollfd pfd = { .fd = fd, .events = POLLIN };
    int iters = ITERS(500000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        poll(&pfd, 1, 0);
    }
    report("poll", iters, now_ns() - start);
    close(fd);
}

static void bench_eventfd(void) {
    int fd = eventfd(0, EFD_NONBLOCK);
    if (fd < 0) { printf("BENCH_SKIP eventfd\n"); return; }
    uint64_t val = 1;
    int iters = ITERS(500000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        write(fd, &val, 8);
        read(fd, &val, 8);
    }
    report("eventfd", iters, now_ns() - start);
    close(fd);
}

static void bench_getdents(void) {
    int iters = ITERS(100000, 1000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        int fd = open("/tmp", O_RDONLY | O_DIRECTORY);
        if (fd < 0) break;
        char buf[1024];
        syscall(SYS_getdents64, fd, buf, sizeof(buf));
        close(fd);
    }
    report("getdents64", iters, now_ns() - start);
}

static void bench_socketpair(void) {
    int iters = ITERS(200000, 2000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        int sv[2];
        socketpair(AF_UNIX, SOCK_STREAM, 0, sv);
        close(sv[0]);
        close(sv[1]);
    }
    report("socketpair", iters, now_ns() - start);
}

static void bench_pipe_pingpong(void) {
    int p1[2], p2[2];
    pipe(p1); pipe(p2);
    pid_t pid = fork();
    if (pid == 0) {
        char c;
        int iters = ITERS(50000, 500);
        for (int i = 0; i < iters; i++) {
            read(p1[0], &c, 1);
            write(p2[1], &c, 1);
        }
        _exit(0);
    }
    char c = 'x';
    int iters = ITERS(50000, 500);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        write(p1[1], &c, 1);
        read(p2[0], &c, 1);
    }
    report("pipe_pingpong", iters, now_ns() - start);
    close(p1[0]); close(p1[1]); close(p2[0]); close(p2[1]);
    waitpid(pid, NULL, 0);
}

static void bench_waitid_nochild(void) {
    int iters = ITERS(500000, 5000);
    siginfo_t si;
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        waitid(P_ALL, 0, &si, WNOHANG | WEXITED);
    }
    report("waitid", iters, now_ns() - start);
}

static void bench_getuid(void) {
    int iters = ITERS(1000000, 10000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        getuid();
    }
    report("getuid", iters, now_ns() - start);
}

/* ── Workload benchmarks (7) ────────────────────────────────────────── */
/* These measure real BusyBox/shell workload patterns, not raw syscalls. */

static void bench_exec_true(void) {
    int iters = ITERS(200, 100);
    int completed = 0;
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        pid_t pid = fork();
        if (pid == 0) {
            execl("/bin/true", "true", NULL);
            _exit(127);
        } else if (pid > 0) {
            waitpid(pid, NULL, 0);
            completed++;
        } else break;
    }
    if (completed > 0)
        report("exec_true", completed, now_ns() - start);
}

static void bench_shell_noop(void) {
    int iters = ITERS(100, 50);
    int completed = 0;
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        pid_t pid = fork();
        if (pid == 0) {
            execl("/bin/sh", "sh", "-c", "true", NULL);
            _exit(127);
        } else if (pid > 0) {
            waitpid(pid, NULL, 0);
            completed++;
        } else break;
    }
    if (completed > 0)
        report("shell_noop", completed, now_ns() - start);
}

/* Kevlar-private SYS_KVLR_SPAWN(path, argv, envp, flags[, fa, attr]).
 * See blog 224 (v1) and blog 227 (v2 file_actions + attrs). */
#ifndef SYS_kvlr_spawn
#define SYS_kvlr_spawn 500
#endif

/* v2 flag bit: enables a5 = file_actions ptr, a6 = attr ptr. */
#define KVLR_SPAWN_F_EXTENDED 1u

#define KVLR_SPAWN_FA_CLOSE 1u
#define KVLR_SPAWN_FA_OPEN  2u
#define KVLR_SPAWN_FA_DUP2  3u

#define KVLR_SPAWN_SETSIGMASK 1u
#define KVLR_SPAWN_SETSIGDEF  2u
#define KVLR_SPAWN_SETPGROUP  4u
#define KVLR_SPAWN_SETSID     8u
#define KVLR_SPAWN_RESETIDS   16u

struct kvlr_spawn_file_action {
    unsigned int op;
    int fd;
    int newfd;
    int oflag;
    unsigned int mode;
    unsigned int _pad;
    const char *path;
};

struct kvlr_spawn_file_actions_hdr {
    unsigned int count;
    unsigned int _pad;
    /* followed by struct kvlr_spawn_file_action actions[count] */
};

struct kvlr_spawn_attr {
    unsigned int flags;
    int pgid;
    unsigned long long sigmask;
    unsigned long long sigdefault;
};

static void bench_exec_true_spawn(void) {
    /* Probe once: if kvlr_spawn isn't supported, skip without burning
     * iterations.  Linux returns -ENOSYS; Kevlar < blog 224 returns -ENOSYS;
     * the SUID/SGID handling on the probe path is harmless because /bin/true
     * is mode 0755 with neither bit set. */
    char *probe_argv[] = { "true", NULL };
    char *probe_envp[] = { NULL };
    long probe = syscall(SYS_kvlr_spawn, "/bin/true", probe_argv, probe_envp, 0);
    if (probe < 0) { printf("BENCH_SKIP exec_true_spawn (kvlr_spawn unsupported)\n"); return; }
    waitpid((pid_t)probe, NULL, 0);

    int iters = ITERS(200, 100);
    int completed = 0;
    char *argv[] = { "true", NULL };
    char *envp[] = { NULL };
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        long pid = syscall(SYS_kvlr_spawn, "/bin/true", argv, envp, 0);
        if (pid < 0) break;
        waitpid((pid_t)pid, NULL, 0);
        completed++;
    }
    if (completed > 0)
        report("exec_true_spawn", completed, now_ns() - start);
}

/* v2 smoke test: spawn /bin/true with an empty file_actions array and a
 * zero-flag attr via KVLR_SPAWN_F_EXTENDED.  Verifies the extended-args
 * parser works end-to-end without exercising any semantic change.
 * Real measurement of file_actions overhead comes once musl's posix_spawn
 * routes through this syscall (blog 227-follow-up). */
static void bench_exec_true_spawn_v2_smoke(void) {
    struct kvlr_spawn_file_actions_hdr empty_fa = { .count = 0, ._pad = 0 };
    struct kvlr_spawn_attr empty_attr = { 0, 0, 0, 0 };

    char *probe_argv[] = { "true", NULL };
    char *probe_envp[] = { NULL };
    long probe = syscall(SYS_kvlr_spawn, "/bin/true", probe_argv, probe_envp,
                         KVLR_SPAWN_F_EXTENDED, &empty_fa, &empty_attr);
    if (probe < 0) {
        printf("BENCH_SKIP exec_true_spawn_v2 (kvlr_spawn v2 unsupported)\n");
        return;
    }
    waitpid((pid_t)probe, NULL, 0);

    int iters = ITERS(200, 100);
    int completed = 0;
    char *argv[] = { "true", NULL };
    char *envp[] = { NULL };
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        long pid = syscall(SYS_kvlr_spawn, "/bin/true", argv, envp,
                           KVLR_SPAWN_F_EXTENDED, &empty_fa, &empty_attr);
        if (pid < 0) break;
        waitpid((pid_t)pid, NULL, 0);
        completed++;
    }
    if (completed > 0)
        report("exec_true_spawn_v2", completed, now_ns() - start);
}

static void bench_shell_noop_spawn(void) {
    char *probe_argv[] = { "sh", "-c", "true", NULL };
    char *probe_envp[] = { NULL };
    long probe = syscall(SYS_kvlr_spawn, "/bin/sh", probe_argv, probe_envp, 0);
    if (probe < 0) { printf("BENCH_SKIP shell_noop_spawn (kvlr_spawn unsupported)\n"); return; }
    waitpid((pid_t)probe, NULL, 0);

    int iters = ITERS(100, 50);
    int completed = 0;
    char *argv[] = { "sh", "-c", "true", NULL };
    char *envp[] = { NULL };
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        long pid = syscall(SYS_kvlr_spawn, "/bin/sh", argv, envp, 0);
        if (pid < 0) break;
        waitpid((pid_t)pid, NULL, 0);
        completed++;
    }
    if (completed > 0)
        report("shell_noop_spawn", completed, now_ns() - start);
}

/* Common helper: run a `/bin/sh -c <cmd>` pipeline N times via SYS_KVLR_SPAWN,
 * report as `<name>_spawn`.  Returns 1 on success, 0 if kvlr_spawn is
 * unsupported (Linux, or pre-224 Kevlar) so the caller can BENCH_SKIP.
 * Skips probing overhead — the outer caller's first iter serves as probe,
 * and an early -1 from syscall() aborts the loop cleanly. */
static int run_shell_pipeline_spawn(const char *name, const char *cmd, int iters) {
    char *probe_argv[] = { "sh", "-c", (char *)cmd, NULL };
    char *probe_envp[] = { NULL };
    long probe = syscall(SYS_kvlr_spawn, "/bin/sh", probe_argv, probe_envp, 0);
    if (probe < 0) {
        printf("BENCH_SKIP %s_spawn (kvlr_spawn unsupported)\n", name);
        return 0;
    }
    waitpid((pid_t)probe, NULL, 0);

    char *argv[] = { "sh", "-c", (char *)cmd, NULL };
    char *envp[] = { NULL };
    int completed = 0;
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        long pid = syscall(SYS_kvlr_spawn, "/bin/sh", argv, envp, 0);
        if (pid < 0) break;
        waitpid((pid_t)pid, NULL, 0);
        completed++;
    }
    if (completed > 0) {
        char buf[64];
        snprintf(buf, sizeof(buf), "%s_spawn", name);
        report(buf, completed, now_ns() - start);
    }
    return 1;
}

static void bench_pipe_grep_spawn(void) {
    int tmpfd = open("/tmp/bench_grep", O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (tmpfd < 0) { printf("BENCH_SKIP pipe_grep_spawn\n"); return; }
    const char *data = "line1 apple\nline2 banana\nline3 apple\n";
    for (int i = 0; i < 100; i++) write(tmpfd, data, strlen(data));
    close(tmpfd);
    run_shell_pipeline_spawn("pipe_grep",
        "grep apple /tmp/bench_grep > /dev/null", ITERS(100, 50));
    unlink("/tmp/bench_grep");
}

static void bench_sed_pipeline_spawn(void) {
    int tmpfd = open("/tmp/bench_sed", O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (tmpfd < 0) { printf("BENCH_SKIP sed_pipeline_spawn\n"); return; }
    for (int i = 0; i < 200; i++) {
        char line[64];
        int len = snprintf(line, sizeof(line), "prefix_item_%d_suffix\n", i);
        write(tmpfd, line, len);
    }
    close(tmpfd);
    run_shell_pipeline_spawn("sed_pipeline",
        "sed 's/prefix/PRE/;s/suffix/SUF/' /tmp/bench_sed > /dev/null",
        ITERS(100, 20));
    unlink("/tmp/bench_sed");
}

static void bench_sort_uniq_spawn(void) {
    int tmpfd = open("/tmp/bench_sort", O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (tmpfd < 0) { printf("BENCH_SKIP sort_uniq_spawn\n"); return; }
    const char *words[] = {"apple","banana","cherry","date","elderberry","fig","grape"};
    for (int i = 0; i < 500; i++) {
        const char *w = words[i % 7];
        write(tmpfd, w, strlen(w));
        write(tmpfd, "\n", 1);
    }
    close(tmpfd);
    run_shell_pipeline_spawn("sort_uniq",
        "sort /tmp/bench_sort | uniq -c | sort -rn > /dev/null",
        ITERS(50, 10));
    unlink("/tmp/bench_sort");
}

static void bench_tar_extract_spawn(void) {
    /* Reuse the tar archive built by the non-_spawn bench (runs first in
     * the suite).  If it isn't there — because bench_tar_extract wasn't
     * run or its cleanup nuked it — we rebuild one here. */
    struct stat st;
    if (stat("/tmp/bench.tar", &st) != 0) {
        mkdir("/tmp/bench_tar_in", 0755);
        for (int i = 0; i < 20; i++) {
            char path[64];
            snprintf(path, sizeof(path), "/tmp/bench_tar_in/f%d", i);
            int fd = open(path, O_CREAT | O_WRONLY, 0644);
            if (fd >= 0) { write(fd, "benchmark data\n", 15); close(fd); }
        }
        pid_t pid = fork();
        if (pid == 0) {
            execl("/bin/sh", "sh", "-c", "tar cf /tmp/bench.tar -C /tmp bench_tar_in", NULL);
            _exit(127);
        }
        waitpid(pid, NULL, 0);
    }
    run_shell_pipeline_spawn("tar_extract",
        "rm -rf /tmp/bench_tar_out_spawn; mkdir /tmp/bench_tar_out_spawn; "
        "tar xf /tmp/bench.tar -C /tmp/bench_tar_out_spawn",
        ITERS(50, 10));
    /* Cleanup */
    pid_t pid = fork();
    if (pid == 0) {
        execl("/bin/sh", "sh", "-c",
              "rm -rf /tmp/bench_tar_in /tmp/bench_tar_out_spawn /tmp/bench.tar", NULL);
        _exit(127);
    }
    waitpid(pid, NULL, 0);
}

static void bench_pipe_grep(void) {
    /* Simulate: echo <data> | grep <pattern> — measures pipe+fork+exec */
    int iters = ITERS(100, 50);
    int completed = 0;
    /* Pre-create a file to grep from (avoids echo overhead in loop) */
    int tmpfd = open("/tmp/bench_grep", O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (tmpfd < 0) { printf("BENCH_SKIP pipe_grep\n"); return; }
    const char *data = "line1 apple\nline2 banana\nline3 apple\n";
    for (int i = 0; i < 100; i++) write(tmpfd, data, strlen(data));
    close(tmpfd);

    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        pid_t pid = fork();
        if (pid == 0) {
            execl("/bin/sh", "sh", "-c", "grep apple /tmp/bench_grep > /dev/null", NULL);
            _exit(127);
        } else if (pid > 0) {
            waitpid(pid, NULL, 0);
            completed++;
        } else break;
    }
    if (completed > 0)
        report("pipe_grep", completed, now_ns() - start);
    unlink("/tmp/bench_grep");
}

static void bench_file_tree(void) {
    /* Create dirs + files, readdir, delete — measures VFS path resolution + inode ops */
    int iters = ITERS(50, 10);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        mkdir("/tmp/bench_tree", 0755);
        mkdir("/tmp/bench_tree/sub", 0755);
        for (int j = 0; j < 10; j++) {
            char path[64];
            snprintf(path, sizeof(path), "/tmp/bench_tree/sub/f%d", j);
            int fd = open(path, O_CREAT | O_WRONLY, 0644);
            if (fd >= 0) { write(fd, "x", 1); close(fd); }
        }
        /* Readdir */
        DIR *d = opendir("/tmp/bench_tree/sub");
        if (d) { while (readdir(d)) {} closedir(d); }
        /* Cleanup */
        for (int j = 0; j < 10; j++) {
            char path[64];
            snprintf(path, sizeof(path), "/tmp/bench_tree/sub/f%d", j);
            unlink(path);
        }
        rmdir("/tmp/bench_tree/sub");
        rmdir("/tmp/bench_tree");
    }
    report("file_tree", iters, now_ns() - start);
}

static void bench_sed_pipeline(void) {
    /* Simulate text processing: sed on a file — measures read+regex+write pattern */
    int tmpfd = open("/tmp/bench_sed", O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (tmpfd < 0) { printf("BENCH_SKIP sed_pipeline\n"); return; }
    for (int i = 0; i < 200; i++) {
        char line[64];
        int len = snprintf(line, sizeof(line), "prefix_item_%d_suffix\n", i);
        write(tmpfd, line, len);
    }
    close(tmpfd);

    int iters = ITERS(100, 20);
    int completed = 0;
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        pid_t pid = fork();
        if (pid == 0) {
            execl("/bin/sh", "sh", "-c", "sed 's/prefix/PRE/;s/suffix/SUF/' /tmp/bench_sed > /dev/null", NULL);
            _exit(127);
        } else if (pid > 0) {
            waitpid(pid, NULL, 0);
            completed++;
        } else break;
    }
    if (completed > 0)
        report("sed_pipeline", completed, now_ns() - start);
    unlink("/tmp/bench_sed");
}

static void bench_sort_uniq(void) {
    /* Sort + uniq pipeline — measures fork+pipe+sort algorithm */
    int tmpfd = open("/tmp/bench_sort", O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (tmpfd < 0) { printf("BENCH_SKIP sort_uniq\n"); return; }
    const char *words[] = {"apple","banana","cherry","date","elderberry","fig","grape"};
    for (int i = 0; i < 500; i++) {
        const char *w = words[i % 7];
        write(tmpfd, w, strlen(w));
        write(tmpfd, "\n", 1);
    }
    close(tmpfd);

    int iters = ITERS(50, 10);
    int completed = 0;
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        pid_t pid = fork();
        if (pid == 0) {
            execl("/bin/sh", "sh", "-c", "sort /tmp/bench_sort | uniq -c | sort -rn > /dev/null", NULL);
            _exit(127);
        } else if (pid > 0) {
            waitpid(pid, NULL, 0);
            completed++;
        } else break;
    }
    if (completed > 0)
        report("sort_uniq", completed, now_ns() - start);
    unlink("/tmp/bench_sort");
}

static void bench_tar_extract(void) {
    /* Create a tar archive and repeatedly extract — measures VFS + decompression */
    mkdir("/tmp/bench_tar_in", 0755);
    for (int i = 0; i < 20; i++) {
        char path[64];
        snprintf(path, sizeof(path), "/tmp/bench_tar_in/f%d", i);
        int fd = open(path, O_CREAT | O_WRONLY, 0644);
        if (fd >= 0) { write(fd, "benchmark data\n", 15); close(fd); }
    }
    /* Create the archive once */
    pid_t pid = fork();
    if (pid == 0) {
        execl("/bin/sh", "sh", "-c", "tar cf /tmp/bench.tar -C /tmp bench_tar_in", NULL);
        _exit(127);
    }
    waitpid(pid, NULL, 0);

    int iters = ITERS(50, 10);
    int completed = 0;
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        pid_t p = fork();
        if (p == 0) {
            execl("/bin/sh", "sh", "-c",
                  "rm -rf /tmp/bench_tar_out; mkdir /tmp/bench_tar_out; "
                  "tar xf /tmp/bench.tar -C /tmp/bench_tar_out", NULL);
            _exit(127);
        } else if (p > 0) {
            waitpid(p, NULL, 0);
            completed++;
        } else break;
    }
    if (completed > 0)
        report("tar_extract", completed, now_ns() - start);

    /* Cleanup */
    pid = fork();
    if (pid == 0) {
        execl("/bin/sh", "sh", "-c", "rm -rf /tmp/bench_tar_in /tmp/bench_tar_out /tmp/bench.tar", NULL);
        _exit(127);
    }
    waitpid(pid, NULL, 0);
}

/* ── Phase 1/2 benchmarks (9): new POSIX features ──────────────────── */

static void bench_statx(void) {
    struct statx stx;
    int iters = ITERS(200000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        syscall(SYS_statx, AT_FDCWD, "/tmp", 0, 0x7ff, &stx);
    }
    report("statx", iters, now_ns() - start);
}

static void bench_getsid(void) {
    int iters = ITERS(1000000, 10000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        syscall(SYS_getsid, 0);
    }
    report("getsid", iters, now_ns() - start);
}

static void bench_getrlimit(void) {
    struct rlimit rl;
    int iters = ITERS(1000000, 10000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        getrlimit(RLIMIT_NOFILE, &rl);
    }
    report("getrlimit", iters, now_ns() - start);
}

static void bench_prlimit64(void) {
    struct rlimit old;
    int iters = ITERS(500000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        /* prlimit64(0, RLIMIT_NOFILE, NULL, &old) — get only */
        syscall(SYS_prlimit64, 0, RLIMIT_NOFILE, NULL, &old);
    }
    report("prlimit64", iters, now_ns() - start);
}

static void bench_fcntl_lock(void) {
    int fd = open("/tmp/bench_lockfile", O_CREAT | O_RDWR, 0644);
    if (fd < 0) { printf("BENCH_SKIP fcntl_lock\n"); return; }
    struct flock fl;
    int iters = ITERS(200000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        fl.l_type = F_WRLCK;
        fl.l_whence = SEEK_SET;
        fl.l_start = 0;
        fl.l_len = 0;
        fl.l_pid = 0;
        fcntl(fd, F_SETLK, &fl);
        fl.l_type = F_UNLCK;
        fcntl(fd, F_SETLK, &fl);
    }
    report("fcntl_lock", iters, now_ns() - start);
    close(fd);
    unlink("/tmp/bench_lockfile");
}

static void bench_setsockopt(void) {
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) { printf("BENCH_SKIP setsockopt\n"); return; }
    int val = 1;
    int iters = ITERS(500000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &val, sizeof(val));
    }
    report("setsockopt", iters, now_ns() - start);
    close(fd);
}

static void bench_getsockopt(void) {
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) { printf("BENCH_SKIP getsockopt\n"); return; }
    int val;
    socklen_t len = sizeof(val);
    int iters = ITERS(500000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        getsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &val, &len);
    }
    report("getsockopt", iters, now_ns() - start);
    close(fd);
}

static void bench_flock(void) {
    int fd = open("/tmp/bench_flockfile", O_CREAT | O_RDWR, 0644);
    if (fd < 0) { printf("BENCH_SKIP flock\n"); return; }
    int iters = ITERS(200000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        flock(fd, LOCK_EX);
        flock(fd, LOCK_UN);
    }
    report("flock", iters, now_ns() - start);
    close(fd);
    unlink("/tmp/bench_flockfile");
}

static void bench_setrlimit(void) {
    struct rlimit rl;
    getrlimit(RLIMIT_CORE, &rl);
    int iters = ITERS(500000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        setrlimit(RLIMIT_CORE, &rl);
    }
    report("setrlimit", iters, now_ns() - start);
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

    /* M10 benchmarks (8) */
    {"epoll_wait",   bench_epoll,          0},
    {"poll",         bench_poll_null,      0},
    {"eventfd",      bench_eventfd,        0},
    {"getdents64",   bench_getdents,       0},
    {"socketpair",   bench_socketpair,     0},
    {"pipe_pingpong",bench_pipe_pingpong,  0},
    {"waitid",       bench_waitid_nochild, 0},
    {"getuid",       bench_getuid,         0},

    /* Workload benchmarks (7) */
    {"exec_true",    bench_exec_true,      0},
    {"shell_noop",   bench_shell_noop,     0},
    {"fork_exit_kvlr",   bench_fork_kvlr,        0},
    {"exec_true_spawn",  bench_exec_true_spawn,  0},
    {"exec_true_spawn_v2", bench_exec_true_spawn_v2_smoke, 0},
    {"shell_noop_spawn", bench_shell_noop_spawn, 0},
    {"pipe_grep",    bench_pipe_grep,      0},
    {"pipe_grep_spawn",    bench_pipe_grep_spawn,      0},
    {"file_tree",    bench_file_tree,      0},
    {"sed_pipeline", bench_sed_pipeline,   0},
    {"sed_pipeline_spawn", bench_sed_pipeline_spawn,   0},
    {"sort_uniq",    bench_sort_uniq,      0},
    {"sort_uniq_spawn",    bench_sort_uniq_spawn,      0},
    {"tar_extract",  bench_tar_extract,    0},
    {"tar_extract_spawn",  bench_tar_extract_spawn,    0},

    /* Phase 1/2 benchmarks (9) */
    {"statx",        bench_statx,          0},
    {"getsid",       bench_getsid,         0},
    {"getrlimit",    bench_getrlimit,      0},
    {"prlimit64",    bench_prlimit64,      0},
    {"setrlimit",    bench_setrlimit,      0},
    {"fcntl_lock",   bench_fcntl_lock,     0},
    {"flock",        bench_flock,          0},
    {"setsockopt",   bench_setsockopt,     0},
    {"getsockopt",   bench_getsockopt,     0},

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
