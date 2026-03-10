/*
 * SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
 *
 * mini_systemd: Integration test exercising the same codepaths as systemd
 * PID 1 initialization. Tests epoll, signalfd, timerfd, eventfd, mount,
 * AF_UNIX sockets, prctl, capabilities, and fork/exec.
 *
 * Usage: /bin/mini-systemd
 * Output: TEST_PASS <name> or TEST_FAIL <name> <reason>
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/mount.h>
#include <sys/prctl.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/timerfd.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

/* Linux signalfd / eventfd headers. */
#include <sys/signalfd.h>
#include <sys/eventfd.h>

/* Linux capabilities (not in musl headers, define manually). */
#ifndef _LINUX_CAPABILITY_VERSION_3
#define _LINUX_CAPABILITY_VERSION_3 0x20080522
struct __user_cap_header_struct {
    uint32_t version;
    int pid;
};
struct __user_cap_data_struct {
    uint32_t effective;
    uint32_t permitted;
    uint32_t inheritable;
};
#endif

static int pass_count = 0;
static int fail_count = 0;

#define TEST_PASS(name) do { printf("TEST_PASS %s\n", name); pass_count++; } while(0)
#define TEST_FAIL(name, reason) do { printf("TEST_FAIL %s: %s (errno=%d)\n", name, reason, errno); fail_count++; } while(0)

/* ── Init mode: mount pseudo-filesystems if running as PID 1 ─────── */

static void init_setup(void) {
    if (getpid() != 1)
        return;

    /* Mount /proc. */
    mkdir("/proc", 0555);
    if (mount("proc", "/proc", "proc", 0, NULL) < 0 && errno != EBUSY)
        printf("warning: mount /proc failed: %s\n", strerror(errno));

    /* Mount /sys. */
    mkdir("/sys", 0555);
    if (mount("sysfs", "/sys", "sysfs", 0, NULL) < 0 && errno != EBUSY)
        printf("warning: mount /sys failed: %s\n", strerror(errno));

    /* Mount /tmp. */
    mkdir("/tmp", 01777);
    if (mount("tmpfs", "/tmp", "tmpfs", 0, NULL) < 0 && errno != EBUSY)
        printf("warning: mount /tmp failed: %s\n", strerror(errno));

    /* Mount /run (systemd expects this). */
    mkdir("/run", 0755);
    if (mount("tmpfs", "/run", "tmpfs", 0, NULL) < 0 && errno != EBUSY)
        printf("warning: mount /run failed: %s\n", strerror(errno));
}

/* ── Test: mount pseudo-filesystems ──────────────────────────────── */

static void test_mount(void) {
    /* Check /proc/self/stat is readable. */
    int fd = open("/proc/self/stat", O_RDONLY);
    if (fd < 0) {
        TEST_FAIL("mount_proc", "cannot open /proc/self/stat");
        return;
    }
    char buf[256];
    int n = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (n <= 0) {
        TEST_FAIL("mount_proc", "empty /proc/self/stat");
        return;
    }
    buf[n] = '\0';
    /* Should start with our PID. */
    TEST_PASS("mount_proc");

    /* Check /proc/meminfo. */
    fd = open("/proc/meminfo", O_RDONLY);
    if (fd < 0) {
        TEST_FAIL("mount_meminfo", "cannot open /proc/meminfo");
        return;
    }
    n = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (n > 0) {
        TEST_PASS("mount_meminfo");
    } else {
        TEST_FAIL("mount_meminfo", "empty /proc/meminfo");
    }

    /* Check /proc/mounts. */
    fd = open("/proc/mounts", O_RDONLY);
    if (fd >= 0) {
        n = read(fd, buf, sizeof(buf) - 1);
        close(fd);
        if (n > 0) {
            TEST_PASS("mount_mounts");
        } else {
            TEST_FAIL("mount_mounts", "empty /proc/mounts");
        }
    } else {
        TEST_FAIL("mount_mounts", "cannot open /proc/mounts");
    }
}

/* ── Test: epoll ─────────────────────────────────────────────────── */

static int test_epoll(void) {
    int epfd = epoll_create1(EPOLL_CLOEXEC);
    if (epfd < 0) {
        TEST_FAIL("epoll_create", strerror(errno));
        return -1;
    }
    TEST_PASS("epoll_create");
    return epfd;
}

/* ── Test: signalfd ──────────────────────────────────────────────── */

static int test_signalfd(int epfd) {
    sigset_t mask;
    sigemptyset(&mask);
    sigaddset(&mask, SIGCHLD);
    sigaddset(&mask, SIGTERM);
    sigprocmask(SIG_BLOCK, &mask, NULL);

    int sfd = signalfd(-1, &mask, SFD_NONBLOCK | SFD_CLOEXEC);
    if (sfd < 0) {
        TEST_FAIL("signalfd", strerror(errno));
        return -1;
    }

    struct epoll_event ev = { .events = EPOLLIN, .data.fd = sfd };
    if (epoll_ctl(epfd, EPOLL_CTL_ADD, sfd, &ev) < 0) {
        TEST_FAIL("signalfd_epoll", strerror(errno));
        close(sfd);
        return -1;
    }

    TEST_PASS("signalfd");
    return sfd;
}

/* ── Test: timerfd ───────────────────────────────────────────────── */

static int test_timerfd(int epfd) {
    int tfd = timerfd_create(CLOCK_MONOTONIC, TFD_CLOEXEC);
    if (tfd < 0) {
        TEST_FAIL("timerfd_create", strerror(errno));
        return -1;
    }

    /* Arm a 100ms one-shot timer. */
    struct itimerspec its = {
        .it_value = { .tv_sec = 0, .tv_nsec = 100000000 }
    };
    if (timerfd_settime(tfd, 0, &its, NULL) < 0) {
        TEST_FAIL("timerfd_settime", strerror(errno));
        close(tfd);
        return -1;
    }

    struct epoll_event ev = { .events = EPOLLIN, .data.fd = tfd };
    if (epoll_ctl(epfd, EPOLL_CTL_ADD, tfd, &ev) < 0) {
        TEST_FAIL("timerfd_epoll", strerror(errno));
        close(tfd);
        return -1;
    }

    TEST_PASS("timerfd");
    return tfd;
}

/* ── Test: eventfd ───────────────────────────────────────────────── */

static int test_eventfd(int epfd) {
    int efd = eventfd(0, EFD_CLOEXEC | EFD_NONBLOCK);
    if (efd < 0) {
        TEST_FAIL("eventfd", strerror(errno));
        return -1;
    }

    struct epoll_event ev = { .events = EPOLLIN, .data.fd = efd };
    if (epoll_ctl(epfd, EPOLL_CTL_ADD, efd, &ev) < 0) {
        TEST_FAIL("eventfd_epoll", strerror(errno));
        close(efd);
        return -1;
    }

    /* Write to eventfd, should trigger epoll. */
    uint64_t val = 1;
    if (write(efd, &val, sizeof(val)) != sizeof(val)) {
        TEST_FAIL("eventfd_write", strerror(errno));
        close(efd);
        return -1;
    }

    TEST_PASS("eventfd");
    return efd;
}

/* ── Test: prctl ─────────────────────────────────────────────────── */

static void test_prctl(void) {
    /* PR_SET_NAME / PR_GET_NAME. */
    if (prctl(PR_SET_NAME, "mini-systemd", 0, 0, 0) < 0) {
        TEST_FAIL("prctl_set_name", strerror(errno));
        return;
    }

    char name[16] = {0};
    if (prctl(PR_GET_NAME, name, 0, 0, 0) < 0) {
        TEST_FAIL("prctl_get_name", strerror(errno));
        return;
    }

    if (strcmp(name, "mini-systemd") != 0) {
        TEST_FAIL("prctl_name_match", name);
        return;
    }
    TEST_PASS("prctl_name");

    /* PR_SET_CHILD_SUBREAPER. */
    if (prctl(PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) < 0) {
        TEST_FAIL("prctl_subreaper", strerror(errno));
        return;
    }
    TEST_PASS("prctl_subreaper");
}

/* ── Test: capabilities ──────────────────────────────────────────── */

static void test_capabilities(void) {
    struct __user_cap_header_struct hdr = {
        .version = _LINUX_CAPABILITY_VERSION_3,
        .pid = 0
    };
    struct __user_cap_data_struct data[2];
    memset(data, 0, sizeof(data));

    long rc = syscall(125 /* __NR_capget */, &hdr, data);
    if (rc < 0) {
        TEST_FAIL("capget", strerror(errno));
        return;
    }

    if (data[0].effective != 0xFFFFFFFF || data[0].permitted != 0xFFFFFFFF) {
        TEST_FAIL("capget_values", "expected all caps");
        return;
    }
    TEST_PASS("capabilities");
}

/* ── Test: UID/GID tracking ──────────────────────────────────────── */

static void test_uid_gid(void) {
    if (getuid() != 0 || geteuid() != 0 || getgid() != 0 || getegid() != 0) {
        TEST_FAIL("uid_gid", "expected all 0 (root)");
        return;
    }
    TEST_PASS("uid_gid");
}

/* ── Test: AF_UNIX socket ────────────────────────────────────────── */

static void test_unix_socket(void) {
    int sv[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0) {
        TEST_FAIL("unix_socketpair", strerror(errno));
        return;
    }

    const char msg[] = "hello-unix";
    if (write(sv[0], msg, sizeof(msg)) != sizeof(msg)) {
        TEST_FAIL("unix_write", strerror(errno));
        close(sv[0]); close(sv[1]);
        return;
    }

    char buf[32] = {0};
    int n = read(sv[1], buf, sizeof(buf));
    close(sv[0]);
    close(sv[1]);

    if (n != sizeof(msg) || strcmp(buf, msg) != 0) {
        TEST_FAIL("unix_read", "data mismatch");
        return;
    }
    TEST_PASS("unix_socket");
}

/* ── Test: fork + exec + wait ────────────────────────────────────── */

static void test_fork_exec(void) {
    pid_t child = fork();
    if (child < 0) {
        TEST_FAIL("fork", strerror(errno));
        return;
    }

    if (child == 0) {
        /* Child: just exit successfully. */
        _exit(42);
    }

    int status = 0;
    pid_t w = waitpid(child, &status, 0);
    if (w != child) {
        TEST_FAIL("waitpid", strerror(errno));
        return;
    }

    if (!WIFEXITED(status) || WEXITSTATUS(status) != 42) {
        TEST_FAIL("fork_exec_status", "expected exit(42)");
        return;
    }
    TEST_PASS("fork_exec");
}

/* ── Test: epoll main loop (integrated) ──────────────────────────── */

static void test_epoll_loop(int epfd, int sfd, int tfd, int efd) {
    int saw_timer = 0;
    int saw_eventfd = 0;

    /* Wait up to 500ms for events. */
    for (int iter = 0; iter < 10; iter++) {
        struct epoll_event events[4];
        int n = epoll_wait(epfd, events, 4, 100);

        for (int i = 0; i < n; i++) {
            if (events[i].data.fd == tfd) {
                uint64_t exp;
                read(tfd, &exp, sizeof(exp));
                saw_timer = 1;
            } else if (events[i].data.fd == efd) {
                uint64_t val;
                read(efd, &val, sizeof(val));
                saw_eventfd = 1;
            } else if (events[i].data.fd == sfd) {
                struct signalfd_siginfo si;
                read(sfd, &si, sizeof(si));
            }
        }

        if (saw_timer && saw_eventfd)
            break;
    }

    if (saw_eventfd) {
        TEST_PASS("epoll_eventfd");
    } else {
        TEST_FAIL("epoll_eventfd", "eventfd not triggered via epoll");
    }

    if (saw_timer) {
        TEST_PASS("epoll_timerfd");
    } else {
        TEST_FAIL("epoll_timerfd", "timerfd not triggered via epoll");
    }
}

/* ── Main ────────────────────────────────────────────────────────── */

int main(void) {
    printf("mini-systemd: M4 integration test starting\n");

    init_setup();

    /* Phase 4: Mount tests. */
    test_mount();

    /* Phase 5: Process management. */
    test_prctl();
    test_capabilities();
    test_uid_gid();

    /* Phase 1: Epoll. */
    int epfd = test_epoll();

    /* Phase 2: Event sources. */
    int sfd = (epfd >= 0) ? test_signalfd(epfd) : -1;
    int tfd = (epfd >= 0) ? test_timerfd(epfd) : -1;
    int efd = (epfd >= 0) ? test_eventfd(epfd) : -1;

    /* Phase 3: Unix sockets. */
    test_unix_socket();

    /* Fork + exec. */
    test_fork_exec();

    /* Integrated epoll loop. */
    if (epfd >= 0) {
        test_epoll_loop(epfd, sfd, tfd, efd);
    }

    /* Cleanup. */
    if (sfd >= 0) close(sfd);
    if (tfd >= 0) close(tfd);
    if (efd >= 0) close(efd);
    if (epfd >= 0) close(epfd);

    /* Summary. */
    printf("\nmini-systemd: %d passed, %d failed\n", pass_count, fail_count);
    if (fail_count == 0) {
        printf("TEST_PASS mini_systemd_all\n");
    } else {
        printf("TEST_FAIL mini_systemd_all: %d tests failed\n", fail_count);
    }

    return fail_count > 0 ? 1 : 0;
}
