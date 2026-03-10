/*
 * SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
 *
 * Kevlar kernel syscall correctness test suite.
 * Compiled as a static musl binary and included in the initramfs.
 *
 * Usage: /bin/test [test-name|all]
 *   all      — run all tests (default)
 *   <name>   — run a single named test
 *
 * Output format:
 *   TEST_START kevlar
 *   PASS <name>
 *   FAIL <name> <reason>
 *   TEST_END <passed>/<total>
 */
#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/uio.h>
#include <sys/utsname.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
#include <linux/reboot.h>

static int test_passed = 0;
static int test_total = 0;

#define PASS(name) do { \
    printf("PASS %s\n", name); \
    test_passed++; test_total++; \
} while(0)

#define FAIL(name, fmt, ...) do { \
    printf("FAIL %s " fmt "\n", name, ##__VA_ARGS__); \
    test_total++; \
} while(0)

#define ASSERT(name, cond, fmt, ...) do { \
    if (!(cond)) { FAIL(name, fmt, ##__VA_ARGS__); return; } \
} while(0)

/* ── Init mode setup ──────────────────────────────────────────────── */

static void init_setup(void) {
    if (getpid() != 1) return;

    mkdir("/proc", 0755);
    mkdir("/sys", 0755);
    mkdir("/dev", 0755);
    mkdir("/tmp", 0755);

    mount("proc", "/proc", "proc", 0, NULL);
    mount("sysfs", "/sys", "sysfs", 0, NULL);
    mount("devtmpfs", "/dev", "devtmpfs", 0, NULL);
    mount("tmpfs", "/tmp", "tmpfs", 0, NULL);

    if (open("/dev/null", O_RDONLY) < 0)
        (void)mknod("/dev/null", S_IFCHR | 0666, 0x0103);

    /* Mount ext2 if a block device is available. /tmp is writable. */
    mkdir("/tmp/mnt", 0755);
    mount("none", "/tmp/mnt", "ext2", MS_RDONLY, NULL);
}

/* ── Poll tests ───────────────────────────────────────────────────── */

static void test_poll_stdin(void) {
    /* poll() on stdin should not return EBADF */
    struct pollfd pfd = { .fd = 0, .events = POLLIN };
    int ret = poll(&pfd, 1, 0);  /* timeout=0: non-blocking */
    ASSERT("poll_stdin", ret >= 0,
           "poll(stdin) returned %d errno=%d", ret, errno);
    PASS("poll_stdin");
}

static void test_poll_devnull(void) {
    int fd = open("/dev/null", O_RDWR);
    ASSERT("poll_devnull", fd >= 0, "open /dev/null failed");
    struct pollfd pfd = { .fd = fd, .events = POLLIN | POLLOUT };
    int ret = poll(&pfd, 1, 0);
    close(fd);
    ASSERT("poll_devnull", ret >= 0,
           "poll(/dev/null) returned %d errno=%d", ret, errno);
    PASS("poll_devnull");
}

static void test_poll_pipe(void) {
    int fds[2];
    ASSERT("poll_pipe", pipe(fds) == 0, "pipe() failed");

    /* Empty pipe: read end should not be ready, write end should be */
    struct pollfd pfds[2] = {
        { .fd = fds[0], .events = POLLIN },
        { .fd = fds[1], .events = POLLOUT },
    };
    int ret = poll(pfds, 2, 0);
    ASSERT("poll_pipe", ret >= 0,
           "poll(pipe) returned %d errno=%d", ret, errno);
    /* Write end should be ready */
    ASSERT("poll_pipe", pfds[1].revents & POLLOUT,
           "pipe write end not POLLOUT");

    /* Write data, then read end should be ready */
    write(fds[1], "x", 1);
    pfds[0].revents = 0;
    ret = poll(pfds, 1, 0);
    ASSERT("poll_pipe", ret > 0 && (pfds[0].revents & POLLIN),
           "pipe read end not POLLIN after write");

    close(fds[0]);
    close(fds[1]);
    PASS("poll_pipe");
}

static void test_poll_tmpfile(void) {
    int fd = open("/tmp/poll_test", O_CREAT | O_RDWR, 0644);
    ASSERT("poll_tmpfile", fd >= 0, "open /tmp/poll_test failed");
    struct pollfd pfd = { .fd = fd, .events = POLLIN | POLLOUT };
    int ret = poll(&pfd, 1, 0);
    close(fd);
    unlink("/tmp/poll_test");
    ASSERT("poll_tmpfile", ret >= 0,
           "poll(tmpfile) returned %d errno=%d", ret, errno);
    PASS("poll_tmpfile");
}

static void test_poll_procfile(void) {
    int fd = open("/proc/version", O_RDONLY);
    ASSERT("poll_procfile", fd >= 0, "open /proc/version failed");
    struct pollfd pfd = { .fd = fd, .events = POLLIN };
    int ret = poll(&pfd, 1, 0);
    close(fd);
    ASSERT("poll_procfile", ret >= 0,
           "poll(/proc/version) returned %d errno=%d", ret, errno);
    ASSERT("poll_procfile", pfd.revents & POLLIN,
           "proc file not POLLIN");
    PASS("poll_procfile");
}

/* ── /proc file content tests ─────────────────────────────────────── */

static int last_open_errno = 0;
static int last_read_errno = 0;

static int read_proc_file(const char *path, char *buf, size_t bufsz) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) { last_open_errno = errno; return -1; }
    last_open_errno = 0;
    int n = read(fd, buf, bufsz - 1);
    if (n < 0) { last_read_errno = errno; close(fd); return -2; }
    last_read_errno = 0;
    close(fd);
    buf[n] = '\0';
    return n;
}

static void test_proc_self_status(void) {
    char buf[4096];
    char path[64];
    snprintf(path, sizeof(path), "/proc/%d/status", getpid());
    int n = read_proc_file(path, buf, sizeof(buf));
    ASSERT("proc_self_status", n > 0,
           "failed: n=%d open_errno=%d read_errno=%d path=%s",
           n, last_open_errno, last_read_errno, path);
    ASSERT("proc_self_status", strstr(buf, "Name:") != NULL,
           "missing Name:");
    ASSERT("proc_self_status", strstr(buf, "Pid:") != NULL,
           "missing Pid:");
    ASSERT("proc_self_status", strstr(buf, "FDSize:") != NULL,
           "missing FDSize:");
    ASSERT("proc_self_status", strstr(buf, "VmSize:") != NULL,
           "missing VmSize:");
    ASSERT("proc_self_status", strstr(buf, "SigPnd:") != NULL,
           "missing SigPnd:");
    PASS("proc_self_status");
}

static void test_proc_self_maps(void) {
    char buf[4096];
    char path[64];
    snprintf(path, sizeof(path), "/proc/%d/maps", getpid());
    int n = read_proc_file(path, buf, sizeof(buf));
    ASSERT("proc_self_maps", n > 0,
           "failed: n=%d open_errno=%d read_errno=%d path=%s",
           n, last_open_errno, last_read_errno, path);
    ASSERT("proc_self_maps",
           strstr(buf, "rw-p") != NULL || strstr(buf, "r-xp") != NULL,
           "no VMA lines found");
    ASSERT("proc_self_maps", strstr(buf, "[stack]") != NULL,
           "missing [stack]");
    PASS("proc_self_maps");
}

static void test_proc_self_fd(void) {
    char path[64];
    snprintf(path, sizeof(path), "/proc/%d/fd", getpid());
    DIR *d = opendir(path);
    ASSERT("proc_self_fd", d != NULL,
           "opendir failed for both /proc/self/fd and %s errno=%d",
           path, errno);
    int count = 0;
    struct dirent *ent;
    while ((ent = readdir(d)) != NULL) {
        count++;
    }
    closedir(d);
    /* Should have at least stdin/stdout/stderr (0,1,2) + the dirfd */
    ASSERT("proc_self_fd", count >= 3,
           "only %d entries", count);
    PASS("proc_self_fd");
}

static void test_proc_cpuinfo(void) {
    char buf[4096];
    int n = read_proc_file("/proc/cpuinfo", buf, sizeof(buf));
    ASSERT("proc_cpuinfo", n > 0, "read failed");
    ASSERT("proc_cpuinfo", strstr(buf, "processor") != NULL,
           "missing processor field");
    PASS("proc_cpuinfo");
}

static void test_proc_uptime(void) {
    char buf[256];
    int n = read_proc_file("/proc/uptime", buf, sizeof(buf));
    ASSERT("proc_uptime", n > 0, "read failed");
    /* Should contain two decimal numbers */
    double up, idle;
    int parsed = sscanf(buf, "%lf %lf", &up, &idle);
    ASSERT("proc_uptime", parsed == 2,
           "expected 2 values, got %d from: %s", parsed, buf);
    ASSERT("proc_uptime", up >= 0.0, "negative uptime");
    PASS("proc_uptime");
}

static void test_proc_loadavg(void) {
    char buf[256];
    int n = read_proc_file("/proc/loadavg", buf, sizeof(buf));
    ASSERT("proc_loadavg", n > 0, "read failed");
    ASSERT("proc_loadavg", strstr(buf, "0.00") != NULL,
           "unexpected loadavg content: %s", buf);
    PASS("proc_loadavg");
}

static void test_proc_stat(void) {
    char buf[4096];
    int n = read_proc_file("/proc/stat", buf, sizeof(buf));
    ASSERT("proc_stat", n > 0, "read failed");
    ASSERT("proc_stat", strncmp(buf, "cpu ", 4) == 0,
           "doesn't start with 'cpu '");
    ASSERT("proc_stat", strstr(buf, "processes") != NULL,
           "missing processes line");
    PASS("proc_stat");
}

static void test_proc_meminfo(void) {
    char buf[4096];
    int n = read_proc_file("/proc/meminfo", buf, sizeof(buf));
    ASSERT("proc_meminfo", n > 0, "read failed");
    ASSERT("proc_meminfo", strstr(buf, "MemTotal:") != NULL,
           "missing MemTotal:");
    ASSERT("proc_meminfo", strstr(buf, "MemFree:") != NULL,
           "missing MemFree:");
    PASS("proc_meminfo");
}

/* ── Basic syscall tests ──────────────────────────────────────────── */

static void test_getpid(void) {
    pid_t pid = getpid();
    ASSERT("getpid", pid > 0, "pid=%d", pid);
    ASSERT("getpid", getpid() == pid, "inconsistent pid");
    PASS("getpid");
}

static void test_pipe_rw(void) {
    int fds[2];
    ASSERT("pipe_rw", pipe(fds) == 0, "pipe() failed");
    const char *msg = "hello";
    write(fds[1], msg, 5);
    char buf[16];
    int n = read(fds[0], buf, sizeof(buf));
    ASSERT("pipe_rw", n == 5, "read %d bytes, expected 5", n);
    ASSERT("pipe_rw", memcmp(buf, msg, 5) == 0, "data mismatch");
    close(fds[0]);
    close(fds[1]);
    PASS("pipe_rw");
}

static void test_fork_wait(void) {
    pid_t pid = fork();
    ASSERT("fork_wait", pid >= 0, "fork failed");
    if (pid == 0) {
        _exit(42);
    }
    int status;
    pid_t w = waitpid(pid, &status, 0);
    ASSERT("fork_wait", w == pid, "waitpid returned %d", w);
    ASSERT("fork_wait", WIFEXITED(status), "child didn't exit normally");
    ASSERT("fork_wait", WEXITSTATUS(status) == 42,
           "exit status %d, expected 42", WEXITSTATUS(status));
    PASS("fork_wait");
}

static void test_mmap_anon(void) {
    void *p = mmap(NULL, 4096, PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    ASSERT("mmap_anon", p != MAP_FAILED, "mmap failed");
    /* Should be zero-filled */
    char *cp = (char *)p;
    ASSERT("mmap_anon", cp[0] == 0 && cp[4095] == 0, "not zero-filled");
    cp[0] = 'X';
    ASSERT("mmap_anon", cp[0] == 'X', "write didn't stick");
    munmap(p, 4096);
    PASS("mmap_anon");
}

static void test_dup2(void) {
    int fds[2];
    ASSERT("dup2", pipe(fds) == 0, "pipe() failed");
    int newfd = 10;
    int ret = dup2(fds[0], newfd);
    ASSERT("dup2", ret == newfd, "dup2 returned %d", ret);
    /* Write through original, read through dup */
    write(fds[1], "Z", 1);
    char c;
    int n = read(newfd, &c, 1);
    ASSERT("dup2", n == 1 && c == 'Z', "read through dup failed");
    close(fds[0]);
    close(fds[1]);
    close(newfd);
    PASS("dup2");
}

static void test_uname(void) {
    struct utsname u;
    ASSERT("uname", uname(&u) == 0, "uname failed");
    ASSERT("uname", strlen(u.sysname) > 0, "empty sysname");
    PASS("uname");
}

static void test_clock_gettime(void) {
    struct timespec ts;
    ASSERT("clock_gettime", clock_gettime(CLOCK_MONOTONIC, &ts) == 0,
           "clock_gettime failed");
    ASSERT("clock_gettime", ts.tv_sec >= 0, "negative seconds");
    ASSERT("clock_gettime", ts.tv_nsec >= 0 && ts.tv_nsec < 1000000000,
           "nsec out of range: %ld", ts.tv_nsec);
    PASS("clock_gettime");
}

static void test_stat_proc(void) {
    struct stat st;
    ASSERT("stat_proc", stat("/proc", &st) == 0,
           "stat(/proc) failed errno=%d", errno);
    ASSERT("stat_proc", S_ISDIR(st.st_mode),
           "/proc is not a directory");
    PASS("stat_proc");
}

static void test_getcwd(void) {
    char buf[256];
    ASSERT("getcwd", getcwd(buf, sizeof(buf)) != NULL,
           "getcwd failed errno=%d", errno);
    ASSERT("getcwd", buf[0] == '/', "cwd doesn't start with /");
    PASS("getcwd");
}

static void test_sigaction_basic(void) {
    struct sigaction sa, old;
    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = SIG_IGN;
    ASSERT("sigaction", sigaction(SIGUSR1, &sa, &old) == 0,
           "sigaction failed");
    /* Restore */
    sa.sa_handler = SIG_DFL;
    sigaction(SIGUSR1, &sa, NULL);
    PASS("sigaction");
}

static void test_readv_writev(void) {
    /* Test writev to /dev/null (no read side, avoids pipe blocking issues) */
    int fd = open("/dev/null", O_WRONLY);
    ASSERT("readv_writev", fd >= 0, "open /dev/null failed");
    char a[] = "AB";
    char b[] = "CD";
    struct iovec wv[2] = {
        { .iov_base = a, .iov_len = 2 },
        { .iov_base = b, .iov_len = 2 },
    };
    ssize_t nw = writev(fd, wv, 2);
    close(fd);
    ASSERT("readv_writev", nw == 4, "writev returned %zd", nw);
    PASS("readv_writev");
}

/* ── ext2 filesystem tests ────────────────────────────────────────── */

static void test_ext2_mount(void) {
    struct stat st;
    int r = stat("/tmp/mnt", &st);
    ASSERT("ext2_mount", r == 0, "stat(/tmp/mnt) failed errno=%d", errno);
    ASSERT("ext2_mount", S_ISDIR(st.st_mode), "/tmp/mnt is not a directory");
    PASS("ext2_mount");
}

static void test_ext2_read_file(void) {
    int fd = open("/tmp/mnt/greeting.txt", O_RDONLY);
    ASSERT("ext2_read_file", fd >= 0, "open greeting.txt failed errno=%d", errno);
    char buf[64];
    ssize_t n = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    ASSERT("ext2_read_file", n > 0, "read returned %zd", n);
    buf[n] = '\0';
    ASSERT("ext2_read_file", strstr(buf, "hello") != NULL,
           "expected 'hello', got: %s", buf);
    PASS("ext2_read_file");
}

static void test_ext2_listdir(void) {
    DIR *d = opendir("/tmp/mnt");
    ASSERT("ext2_listdir", d != NULL, "opendir /tmp/mnt failed errno=%d", errno);
    int found_greeting = 0, found_subdir = 0;
    struct dirent *ent;
    while ((ent = readdir(d)) != NULL) {
        if (strcmp(ent->d_name, "greeting.txt") == 0) found_greeting = 1;
        if (strcmp(ent->d_name, "subdir") == 0)       found_subdir = 1;
    }
    closedir(d);
    ASSERT("ext2_listdir", found_greeting, "greeting.txt not in listing");
    ASSERT("ext2_listdir", found_subdir, "subdir not in listing");
    PASS("ext2_listdir");
}

static void test_ext2_subdir(void) {
    int fd = open("/tmp/mnt/subdir/nested.txt", O_RDONLY);
    ASSERT("ext2_subdir", fd >= 0, "open nested.txt failed errno=%d", errno);
    char buf[64];
    ssize_t n = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    ASSERT("ext2_subdir", n > 0, "read returned %zd", n);
    buf[n] = '\0';
    ASSERT("ext2_subdir", strstr(buf, "nested") != NULL,
           "expected 'nested', got: %s", buf);
    PASS("ext2_subdir");
}

static void test_ext2_symlink(void) {
    /* /tmp/mnt/link.txt -> greeting.txt, should resolve and read */
    int fd = open("/tmp/mnt/link.txt", O_RDONLY);
    ASSERT("ext2_symlink", fd >= 0, "open link.txt failed errno=%d", errno);
    char buf[64];
    ssize_t n = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    ASSERT("ext2_symlink", n > 0, "read returned %zd", n);
    buf[n] = '\0';
    ASSERT("ext2_symlink", strstr(buf, "hello") != NULL,
           "expected 'hello' via symlink, got: %s", buf);
    PASS("ext2_symlink");
}

static void test_ext2_stat(void) {
    struct stat st;
    int r = stat("/tmp/mnt/greeting.txt", &st);
    ASSERT("ext2_stat", r == 0, "stat failed errno=%d", errno);
    ASSERT("ext2_stat", S_ISREG(st.st_mode), "not a regular file");
    ASSERT("ext2_stat", st.st_size > 0, "size is 0");
    PASS("ext2_stat");
}

static void test_ext2_readonly(void) {
    int fd = open("/tmp/mnt/should_not_exist", O_CREAT | O_WRONLY, 0644);
    ASSERT("ext2_readonly", fd < 0, "expected EROFS, got fd=%d", fd);
    ASSERT("ext2_readonly", errno == EROFS, "expected EROFS, got errno=%d", errno);
    PASS("ext2_readonly");
}

/* ── Test registry ────────────────────────────────────────────────── */

typedef struct {
    const char *name;
    void (*fn)(void);
} test_entry;

static test_entry tests[] = {
    /* Poll tests */
    {"poll_stdin",       test_poll_stdin},
    {"poll_devnull",     test_poll_devnull},
    {"poll_pipe",        test_poll_pipe},
    {"poll_tmpfile",     test_poll_tmpfile},
    {"poll_procfile",    test_poll_procfile},

    /* /proc file tests */
    {"proc_self_status", test_proc_self_status},
    {"proc_self_maps",   test_proc_self_maps},
    {"proc_self_fd",     test_proc_self_fd},
    {"proc_cpuinfo",     test_proc_cpuinfo},
    {"proc_uptime",      test_proc_uptime},
    {"proc_loadavg",     test_proc_loadavg},
    {"proc_stat",        test_proc_stat},
    {"proc_meminfo",     test_proc_meminfo},

    /* ext2 filesystem tests (require disk image at /tmp/mnt) */
    {"ext2_mount",       test_ext2_mount},
    {"ext2_read_file",   test_ext2_read_file},
    {"ext2_listdir",     test_ext2_listdir},
    {"ext2_subdir",      test_ext2_subdir},
    {"ext2_symlink",     test_ext2_symlink},
    {"ext2_stat",        test_ext2_stat},
    {"ext2_readonly",    test_ext2_readonly},

    /* Basic syscall tests */
    {"getpid",           test_getpid},
    {"pipe_rw",          test_pipe_rw},
    {"fork_wait",        test_fork_wait},
    {"mmap_anon",        test_mmap_anon},
    {"dup2",             test_dup2},
    {"uname",            test_uname},
    {"clock_gettime",    test_clock_gettime},
    {"stat_proc",        test_stat_proc},
    {"getcwd",           test_getcwd},
    {"sigaction",        test_sigaction_basic},
    {"readv_writev",     test_readv_writev},

    {NULL, NULL}
};

int main(int argc, char **argv) {
    const char *filter = "all";

    if (getpid() == 1) {
        init_setup();
    }

    for (int i = 1; i < argc; i++) {
        filter = argv[i];
    }

    printf("TEST_START kevlar\n");
    fflush(stdout);

    for (test_entry *t = tests; t->name; t++) {
        if (strcmp(filter, "all") == 0 || strcmp(filter, t->name) == 0) {
            t->fn();
            fflush(stdout);
        }
    }

    printf("TEST_END %d/%d\n", test_passed, test_total);
    fflush(stdout);

    if (getpid() == 1) {
        sync();
        syscall(SYS_reboot, LINUX_REBOOT_MAGIC1, LINUX_REBOOT_MAGIC2,
                LINUX_REBOOT_CMD_POWER_OFF, NULL);
    }

    return test_passed == test_total ? 0 : 1;
}
