/*
 * dd diagnostic: find exactly where dd hangs on Kevlar.
 * Standalone binary — no dependency on busybox_suite.c.
 */
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/sysmacros.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static long long now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (long long)ts.tv_sec * 1000000000LL + ts.tv_nsec;
}

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
        mknod("/dev/null", S_IFCHR | 0666, makedev(1, 3));
    if (open("/dev/zero", O_RDONLY) < 0)
        mknod("/dev/zero", S_IFCHR | 0666, makedev(1, 5));
}

/* Test 1: raw syscall dd — open /dev/zero, write to tmpfs file */
static int test_raw_dd(int bs, int count) {
    int in_fd = open("/dev/zero", O_RDONLY);
    int out_fd = open("/tmp/dd_raw", O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (in_fd < 0 || out_fd < 0) {
        printf("  FAIL: open failed in=%d out=%d\n", in_fd, out_fd);
        return -1;
    }

    char *buf = malloc(bs);
    if (!buf) { printf("  FAIL: malloc(%d)\n", bs); return -1; }

    long long start = now_ns();
    for (int i = 0; i < count; i++) {
        ssize_t r = read(in_fd, buf, bs);
        if (r != bs) {
            printf("  FAIL: read returned %zd at iter %d (expected %d)\n", r, i, bs);
            free(buf); close(in_fd); close(out_fd); unlink("/tmp/dd_raw");
            return -1;
        }
        ssize_t w = write(out_fd, buf, bs);
        if (w != bs) {
            printf("  FAIL: write returned %zd at iter %d (expected %d)\n", w, i, bs);
            free(buf); close(in_fd); close(out_fd); unlink("/tmp/dd_raw");
            return -1;
        }
    }
    long long elapsed = now_ns() - start;

    struct stat st;
    fstat(out_fd, &st);
    free(buf);
    close(in_fd);
    close(out_fd);

    long expected = (long)bs * count;
    printf("  OK: raw bs=%d count=%d total=%ldB size=%ld time=%lldus\n",
           bs, count, expected, (long)st.st_size, elapsed / 1000);
    unlink("/tmp/dd_raw");
    return (st.st_size == expected) ? 0 : -1;
}

/* Test 2: BusyBox dd via fork+exec */
static int test_busybox_dd(int bs, int count) {
    char cmd[256];
    snprintf(cmd, sizeof(cmd), "dd if=/dev/zero of=/tmp/dd_bb bs=%d count=%d", bs, count);

    long long start = now_ns();
    pid_t pid = fork();
    if (pid == 0) {
        int devnull = open("/dev/null", O_RDWR);
        if (devnull >= 0) {
            dup2(devnull, STDOUT_FILENO);
            dup2(devnull, STDERR_FILENO);
            close(devnull);
        }
        execl("/bin/sh", "sh", "-c", cmd, NULL);
        _exit(127);
    }

    /* Poll with 5-second hard timeout */
    long long deadline = now_ns() + 5000000000LL;
    int status = 0;
    int exited = 0;
    while (now_ns() < deadline) {
        int wr = waitpid(pid, &status, WNOHANG);
        if (wr == pid) { exited = 1; break; }
        if (wr < 0) break;
        struct timespec ts = { .tv_sec = 0, .tv_nsec = 1000000 }; /* 1ms */
        nanosleep(&ts, NULL);
    }

    long long elapsed = now_ns() - start;

    if (!exited) {
        kill(pid, SIGKILL);
        waitpid(pid, NULL, 0);
        printf("  HANG: busybox dd bs=%d count=%d total=%dB (timed out after 5s)\n",
               bs, count, bs * count);
        unlink("/tmp/dd_bb");
        return -1;
    }

    int rc = WIFEXITED(status) ? WEXITSTATUS(status) : -1;
    struct stat st;
    int st_rc = stat("/tmp/dd_bb", &st);
    long expected = (long)bs * count;

    printf("  %s: busybox dd bs=%d count=%d total=%dB rc=%d size=%ld time=%lldus\n",
           (rc == 0 && st_rc == 0 && st.st_size == expected) ? "OK" : "FAIL",
           bs, count, bs * count, rc,
           st_rc == 0 ? (long)st.st_size : -1, elapsed / 1000);
    unlink("/tmp/dd_bb");
    /* Reap orphans */
    while (waitpid(-1, NULL, WNOHANG) > 0) {}
    return rc;
}

int main(void) {
    if (getpid() == 1) init_setup();

    printf("=== dd diagnostic ===\n\n");

    /* Phase 1: Raw syscall dd — isolate kernel from BusyBox */
    printf("Phase 1: Raw read(/dev/zero) + write(tmpfs) syscalls\n");
    test_raw_dd(512, 4);       /* 2KB — matches test_dd_basic */
    test_raw_dd(4096, 1);      /* 4KB */
    test_raw_dd(4096, 16);     /* 64KB */
    test_raw_dd(4096, 64);     /* 256KB */
    test_raw_dd(4096, 256);    /* 1MB */
    test_raw_dd(4096, 1024);   /* 4MB */
    test_raw_dd(65536, 16);    /* 1MB large blocks */

    /* Phase 2: BusyBox dd — vary block size, count=1 */
    printf("\nPhase 2: BusyBox dd, varying block size (count=1)\n");
    for (int bs = 512; bs <= 65536; bs *= 2)
        if (test_busybox_dd(bs, 1) != 0) break;

    /* Phase 3: BusyBox dd — vary count, bs=512 */
    printf("\nPhase 3: BusyBox dd bs=512, varying count\n");
    int counts[] = {1, 4, 8, 16, 32, 64, 128, 256, 512, 0};
    for (int i = 0; counts[i]; i++)
        if (test_busybox_dd(512, counts[i]) != 0) break;

    /* Phase 4: BusyBox dd — vary count, bs=4096 */
    printf("\nPhase 4: BusyBox dd bs=4096, varying count\n");
    int counts2[] = {1, 4, 8, 16, 32, 64, 128, 256, 0};
    for (int i = 0; counts2[i]; i++)
        if (test_busybox_dd(4096, counts2[i]) != 0) break;

    printf("\n=== dd diagnostic complete ===\n");
    fflush(stdout);

    if (getpid() == 1) {
        sync();
        syscall(SYS_reboot, 0xfee1dead, 672274793, 0x4321fedc, NULL);
    }
    return 0;
}
