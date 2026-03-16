/*
 * SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
 *
 * mini_storage: M5 Phase 7 integration test.
 *
 * Exercises all M5 subsystems together: ext2/VirtIO block mount, statfs,
 * statx, utimensat, inotify+epoll, sendfile, exec-from-disk, /proc
 * completeness.
 *
 * Usage: /bin/mini-storage
 * Output: TEST_PASS <name> or TEST_FAIL <name> <reason>
 *
 * Ext2-dependent tests skip cleanly when no disk image is attached.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/inotify.h>
#include <sys/mount.h>
#include <sys/sendfile.h>
#include <sys/stat.h>
#include <sys/statfs.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

/* statx — define manually for older musl that lacks it. */
#ifndef __NR_statx
#define __NR_statx 332
#endif

#ifndef STATX_BASIC_STATS
struct statx_timestamp { int64_t tv_sec; uint32_t tv_nsec; int32_t _pad; };
struct statx {
    uint32_t stx_mask;
    uint32_t stx_blksize;
    uint64_t stx_attributes;
    uint32_t stx_nlink;
    uint32_t stx_uid;
    uint32_t stx_gid;
    uint16_t stx_mode;
    uint16_t _spare0;
    uint64_t stx_ino;
    uint64_t stx_size;
    uint64_t stx_blocks;
    uint64_t stx_attributes_mask;
    struct statx_timestamp stx_atime, stx_btime, stx_ctime, stx_mtime;
    uint32_t stx_rdev_major, stx_rdev_minor;
    uint32_t stx_dev_major, stx_dev_minor;
    uint64_t stx_mnt_id;
    uint32_t stx_dio_mem_align, stx_dio_offset_align;
    uint64_t _spare3[12];
};
#define STATX_BASIC_STATS 0x000007ffU
#endif

#define EXT2_SUPER_MAGIC  0xEF53L
#define TMPFS_MAGIC       0x01021994L

static int pass_count = 0;
static int fail_count = 0;
static int has_disk   = 0;

#define TEST_PASS(name) do { \
    printf("TEST_PASS %s\n", name); pass_count++; } while (0)
#define TEST_FAIL(name, reason) do { \
    printf("TEST_FAIL %s: %s (errno=%d)\n", name, reason, errno); fail_count++; } while (0)
#define TEST_SKIP(name, reason) do { \
    printf("TEST_SKIP %s: %s\n", name, reason); } while (0)

/* ── Init: mount pseudo-filesystems ─────────────────────────────── */

static void init_setup(void) {
    if (getpid() != 1)
        return;
    mkdir("/proc", 0555);
    if (mount("proc", "/proc", "proc", 0, NULL) < 0 && errno != EBUSY)
        printf("warning: mount /proc: %s\n", strerror(errno));
    mkdir("/tmp", 01777);
    if (mount("tmpfs", "/tmp", "tmpfs", 0, NULL) < 0 && errno != EBUSY)
        printf("warning: mount /tmp: %s\n", strerror(errno));
}

/* ── Try to mount ext2 disk ──────────────────────────────────────── */

static void try_mount_disk(void) {
    mkdir("/tmp/mnt", 0755);
    if (mount("none", "/tmp/mnt", "ext2", MS_RDONLY, NULL) < 0) {
        printf("INFO: no ext2 disk attached, skipping disk tests (errno=%d)\n", errno);
        has_disk = 0;
        return;
    }
    has_disk = 1;
    printf("INFO: ext2 disk mounted at /tmp/mnt\n");
}

/* ── Test: statfs returns correct f_type ─────────────────────────── */

static void test_statfs(void) {
    if (!has_disk) {
        TEST_SKIP("statfs_ext2", "no disk");
    } else {
        struct statfs sfs;
        if (statfs("/tmp/mnt", &sfs) < 0) {
            TEST_FAIL("statfs_ext2", strerror(errno));
        } else if ((long)sfs.f_type != EXT2_SUPER_MAGIC) {
            printf("TEST_FAIL statfs_ext2: expected 0x%lx got 0x%lx\n",
                   EXT2_SUPER_MAGIC, (long)sfs.f_type);
            fail_count++;
        } else {
            TEST_PASS("statfs_ext2");
        }
    }

    /* tmpfs magic always available. */
    struct statfs sfs;
    if (statfs("/tmp", &sfs) == 0 && (long)sfs.f_type == TMPFS_MAGIC) {
        TEST_PASS("statfs_tmpfs");
    } else {
        printf("TEST_FAIL statfs_tmpfs: got 0x%lx (errno=%d)\n",
               (long)sfs.f_type, errno);
        fail_count++;
    }
}

/* ── Test: statx returns correct stx_size ───────────────────────── */

static void test_statx(void) {
    if (!has_disk) {
        TEST_SKIP("statx_size", "no disk");
        return;
    }
    struct statx stx;
    long rc = syscall(__NR_statx, AT_FDCWD, "/tmp/mnt/greeting.txt",
                      0, STATX_BASIC_STATS, &stx);
    if (rc < 0) {
        TEST_FAIL("statx_size", strerror(errno));
        return;
    }
    /* disk_image stage writes "hello from ext2\n" = 16 bytes. */
    if (stx.stx_size != 16) {
        printf("TEST_FAIL statx_size: expected 16 got %llu\n",
               (unsigned long long)stx.stx_size);
        fail_count++;
        return;
    }
    TEST_PASS("statx_size");
}

/* ── Test: utimensat is at least a stub that returns 0 ──────────── */

static void test_utimensat(void) {
    const char *path = has_disk ? "/tmp/mnt/greeting.txt" : "/tmp";
    if (utimensat(AT_FDCWD, path, NULL, 0) == 0) {
        TEST_PASS("utimensat_stub");
    } else {
        TEST_FAIL("utimensat_stub", strerror(errno));
    }
}

/* ── Test: inotify + epoll detect IN_CREATE ──────────────────────── */

static void test_inotify_epoll(void) {
    int ifd = inotify_init1(IN_NONBLOCK | IN_CLOEXEC);
    if (ifd < 0) {
        TEST_FAIL("inotify_init", strerror(errno));
        return;
    }

    if (inotify_add_watch(ifd, "/tmp", IN_CREATE) < 0) {
        TEST_FAIL("inotify_add_watch", strerror(errno));
        close(ifd);
        return;
    }

    int epfd = epoll_create1(EPOLL_CLOEXEC);
    if (epfd < 0) {
        TEST_FAIL("inotify_epoll_create", strerror(errno));
        close(ifd);
        return;
    }

    struct epoll_event ev = { .events = EPOLLIN, .data.fd = ifd };
    if (epoll_ctl(epfd, EPOLL_CTL_ADD, ifd, &ev) < 0) {
        TEST_FAIL("inotify_epoll_ctl", strerror(errno));
        close(epfd);
        close(ifd);
        return;
    }

    /* Trigger IN_CREATE. */
    int tfd = open("/tmp/inotify_trigger", O_CREAT | O_WRONLY, 0644);
    if (tfd >= 0)
        close(tfd);

    struct epoll_event events[4];
    int n = epoll_wait(epfd, events, 4, 500);
    close(epfd);

    if (n <= 0) {
        TEST_FAIL("inotify_epoll_event", "no event via epoll");
        close(ifd);
        return;
    }

    char ibuf[sizeof(struct inotify_event) + 256];
    read(ifd, ibuf, sizeof(ibuf));
    close(ifd);

    TEST_PASS("inotify_epoll");
}

/* ── Test: sendfile copies greeting.txt to tmpfs ─────────────────── */

static void test_sendfile(void) {
    if (!has_disk) {
        TEST_SKIP("sendfile_ext2", "no disk");
        return;
    }

    int src = open("/tmp/mnt/greeting.txt", O_RDONLY);
    if (src < 0) {
        TEST_FAIL("sendfile_open_src", strerror(errno));
        return;
    }

    int dst = open("/tmp/sendfile_out", O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (dst < 0) {
        TEST_FAIL("sendfile_open_dst", strerror(errno));
        close(src);
        return;
    }

    ssize_t n = sendfile(dst, src, NULL, 4096);
    close(src);
    close(dst);

    if (n < 0) {
        TEST_FAIL("sendfile_ext2", strerror(errno));
        return;
    }
    if (n != 16) {
        printf("TEST_FAIL sendfile_ext2: expected 16 bytes got %ld\n", (long)n);
        fail_count++;
        return;
    }

    /* Verify content. */
    int check = open("/tmp/sendfile_out", O_RDONLY);
    char buf[32] = {0};
    read(check, buf, sizeof(buf) - 1);
    close(check);

    if (strncmp(buf, "hello from ext2\n", 16) != 0) {
        TEST_FAIL("sendfile_content", "data mismatch");
        return;
    }
    TEST_PASS("sendfile_ext2");
}

/* ── Test: execve a static binary from ext2 disk ────────────────── */

static void test_exec_from_disk(void) {
    if (!has_disk) {
        TEST_SKIP("exec_disk", "no disk");
        return;
    }

    pid_t child = fork();
    if (child < 0) {
        TEST_FAIL("exec_disk_fork", strerror(errno));
        return;
    }
    if (child == 0) {
        char *argv[] = { "/tmp/mnt/hello", NULL };
        char *envp[] = { NULL };
        execve("/tmp/mnt/hello", argv, envp);
        _exit(127);
    }

    int status = 0;
    waitpid(child, &status, 0);
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        printf("TEST_FAIL exec_disk: exit %d (errno=%d)\n",
               WEXITSTATUS(status), errno);
        fail_count++;
        return;
    }
    TEST_PASS("exec_disk");
}

/* ── Test: /proc/self/maps contains [stack] ──────────────────────── */

static void test_proc_maps(void) {
    int fd = open("/proc/self/maps", O_RDONLY);
    if (fd < 0) {
        TEST_FAIL("proc_maps_open", strerror(errno));
        return;
    }
    char buf[4096] = {0};
    int n = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (n <= 0) {
        TEST_FAIL("proc_maps_read", "empty");
        return;
    }
    if (strstr(buf, "[stack]") == NULL) {
        TEST_FAIL("proc_maps_stack", "no [stack] entry");
        return;
    }
    TEST_PASS("proc_maps");
}

/* ── Test: /proc/cpuinfo contains "processor" ───────────────────── */

static void test_proc_cpuinfo(void) {
    int fd = open("/proc/cpuinfo", O_RDONLY);
    if (fd < 0) {
        TEST_FAIL("proc_cpuinfo_open", strerror(errno));
        return;
    }
    char buf[1024] = {0};
    int n = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (n <= 0) {
        TEST_FAIL("proc_cpuinfo_read", "empty");
        return;
    }
    if (strstr(buf, "processor") == NULL) {
        TEST_FAIL("proc_cpuinfo_field", "no 'processor' field");
        return;
    }
    TEST_PASS("proc_cpuinfo");
}

/* ── Main ────────────────────────────────────────────────────────── */

int main(void) {
    printf("mini-storage: M5 Phase 7 integration test\n");

    init_setup();
    try_mount_disk();

    test_statfs();
    test_statx();
    test_utimensat();
    test_inotify_epoll();
    test_sendfile();
    test_exec_from_disk();
    test_proc_maps();
    test_proc_cpuinfo();

    printf("\nmini-storage: %d passed, %d failed\n", pass_count, fail_count);
    if (fail_count == 0) {
        printf("TEST_PASS mini_storage_all\n");
    } else {
        printf("TEST_FAIL mini_storage_all: %d tests failed\n", fail_count);
    }
    printf("TEST_END\n");

    return fail_count > 0 ? 1 : 0;
}
