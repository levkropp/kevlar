// Ext4 write diagnostic tool for Kevlar.
// Statically linked with musl — runs without any dynamic libraries.
// Tests every file write mechanism to find what's broken on ext4.
//
// Build: musl-gcc -static -O2 -o dyntest dyntest.c
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>
#include <sys/wait.h>
#include <sys/stat.h>
#include <sys/mman.h>
#include <stdarg.h>
#include <time.h>
#include <sys/uio.h>
#include <sys/sendfile.h>

static int PASS = 0, FAIL = 0;

static void msg(const char *s) { write(2, s, strlen(s)); }

static void msgf(const char *fmt, ...) {
    char buf[512];
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    write(2, buf, n);
}

static void pass(const char *name) { msgf("TEST_PASS %s\n", name); PASS++; }
static void fail(const char *name, const char *reason) {
    msgf("TEST_FAIL %s (%s)\n", name, reason); FAIL++;
}

// Verify a file has expected size and content prefix
static int verify_file(const char *path, int expected_size, const char *expected_prefix) {
    struct stat st;
    if (stat(path, &st) != 0) return -1;
    if (st.st_size != expected_size) return -2;
    if (expected_prefix) {
        int fd = open(path, O_RDONLY);
        if (fd < 0) return -3;
        char buf[256];
        int n = read(fd, buf, sizeof(buf) - 1);
        close(fd);
        if (n < 0) return -4;
        buf[n] = 0;
        if (strncmp(buf, expected_prefix, strlen(expected_prefix)) != 0) return -5;
    }
    return 0;
}

// Get monotonic time in microseconds
static long long now_us(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (long long)ts.tv_sec * 1000000 + ts.tv_nsec / 1000;
}

// Test directory — use ext4 mount point
static const char *TESTDIR = "/var/ext4test";

int main(int argc, char **argv) {
    msg("=== ext4 write diagnostic ===\n");

    // Create test directory
    mkdir(TESTDIR, 0755);

    char path[256];

    // ── Test 1: Basic write() ──
    {
        snprintf(path, sizeof(path), "%s/test_write", TESTDIR);
        int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
        if (fd < 0) { fail("basic_write", "open failed"); goto t2; }
        const char *data = "Hello ext4 write!\n";
        int n = write(fd, data, strlen(data));
        close(fd);
        if (n != (int)strlen(data)) { fail("basic_write", "short write"); goto t2; }
        int rc = verify_file(path, strlen(data), "Hello ext4");
        if (rc == 0) pass("basic_write");
        else { char r[64]; snprintf(r, 64, "verify=%d", rc); fail("basic_write", r); }
    }
t2:

    // ── Test 2: Large write (64KB) ──
    {
        snprintf(path, sizeof(path), "%s/test_large", TESTDIR);
        int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
        if (fd < 0) { fail("large_write", "open failed"); goto t3; }
        char buf[4096];
        memset(buf, 'A', sizeof(buf));
        int total = 0;
        for (int i = 0; i < 16; i++) {
            int n = write(fd, buf, sizeof(buf));
            if (n <= 0) { fail("large_write", "write error"); close(fd); goto t3; }
            total += n;
        }
        close(fd);
        struct stat st;
        stat(path, &st);
        if (st.st_size == 65536) pass("large_write_64k");
        else { char r[64]; snprintf(r, 64, "size=%ld", (long)st.st_size); fail("large_write_64k", r); }
    }
t3:

    // ── Test 3: writev() ──
    {
        snprintf(path, sizeof(path), "%s/test_writev", TESTDIR);
        int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
        if (fd < 0) { fail("writev", "open failed"); goto t4; }
        struct iovec iov[3];
        iov[0].iov_base = "part1 ";
        iov[0].iov_len = 6;
        iov[1].iov_base = "part2 ";
        iov[1].iov_len = 6;
        iov[2].iov_base = "part3\n";
        iov[2].iov_len = 6;
        int n = writev(fd, iov, 3);
        close(fd);
        if (n == 18 && verify_file(path, 18, "part1 part2 part3") == 0)
            pass("writev");
        else { char r[64]; snprintf(r, 64, "n=%d", n); fail("writev", r); }
    }
t4:

    // ── Test 4: pwrite() ──
    {
        snprintf(path, sizeof(path), "%s/test_pwrite", TESTDIR);
        int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
        if (fd < 0) { fail("pwrite", "open failed"); goto t5; }
        // Write "AAAA" at offset 0, "BBBB" at offset 100
        pwrite(fd, "AAAA", 4, 0);
        pwrite(fd, "BBBB", 4, 100);
        close(fd);
        struct stat st;
        stat(path, &st);
        if (st.st_size == 104) pass("pwrite");
        else { char r[64]; snprintf(r, 64, "size=%ld", (long)st.st_size); fail("pwrite", r); }
    }
t5:

    // ── Test 5: ftruncate (extend) ──
    {
        snprintf(path, sizeof(path), "%s/test_ftrunc", TESTDIR);
        int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
        if (fd < 0) { fail("ftruncate", "open failed"); goto t6; }
        write(fd, "data", 4);
        int rc = ftruncate(fd, 1024);
        close(fd);
        struct stat st;
        stat(path, &st);
        if (rc == 0 && st.st_size == 1024) pass("ftruncate_extend");
        else { char r[64]; snprintf(r, 64, "rc=%d size=%ld", rc, (long)st.st_size); fail("ftruncate_extend", r); }
    }
t6:

    // ── Test 6: O_APPEND ──
    {
        snprintf(path, sizeof(path), "%s/test_append", TESTDIR);
        int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
        write(fd, "first", 5);
        close(fd);
        fd = open(path, O_WRONLY | O_APPEND);
        if (fd < 0) { fail("append", "open failed"); goto t7; }
        write(fd, "second", 6);
        close(fd);
        if (verify_file(path, 11, "firstsecond") == 0) pass("append");
        else fail("append", "content mismatch");
    }
t7:

    // ── Test 7: mmap MAP_SHARED write ──
    {
        snprintf(path, sizeof(path), "%s/test_mmap_write", TESTDIR);
        int fd = open(path, O_RDWR | O_CREAT | O_TRUNC, 0644);
        if (fd < 0) { fail("mmap_write", "open failed"); goto t8; }
        ftruncate(fd, 4096);
        void *map = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
        if (map == MAP_FAILED) {
            fail("mmap_write", "mmap failed");
            close(fd);
            goto t8;
        }
        memcpy(map, "mmap_data_here!", 15);
        msync(map, 4096, MS_SYNC);
        munmap(map, 4096);
        close(fd);
        if (verify_file(path, 4096, "mmap_data_here!") == 0) pass("mmap_write");
        else fail("mmap_write", "content mismatch");
    }
t8:

    // ── Test 8: sendfile() (how apk extracts archives) ──
    {
        // Create a source file first
        snprintf(path, sizeof(path), "%s/test_sendfile_src", TESTDIR);
        int src_fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
        char srcdata[1024];
        memset(srcdata, 'S', sizeof(srcdata));
        memcpy(srcdata, "SENDFILE_TEST", 13);
        write(src_fd, srcdata, sizeof(srcdata));
        close(src_fd);

        src_fd = open(path, O_RDONLY);
        char dstpath[256];
        snprintf(dstpath, sizeof(dstpath), "%s/test_sendfile_dst", TESTDIR);
        int dst_fd = open(dstpath, O_WRONLY | O_CREAT | O_TRUNC, 0644);
        if (src_fd < 0 || dst_fd < 0) {
            fail("sendfile", "open failed");
            if (src_fd >= 0) close(src_fd);
            if (dst_fd >= 0) close(dst_fd);
            goto t9;
        }
        // Use copy_file_range or sendfile
        off_t off = 0;
        ssize_t copied = sendfile(dst_fd, src_fd, &off, 1024);
        close(src_fd);
        close(dst_fd);
        if (copied == 1024 && verify_file(dstpath, 1024, "SENDFILE_TEST") == 0)
            pass("sendfile");
        else {
            char r[64];
            snprintf(r, 64, "copied=%zd errno=%d", copied, errno);
            fail("sendfile", r);
        }
    }
t9:

    // ── Test 9: Benchmark — sequential write throughput ──
    {
        snprintf(path, sizeof(path), "%s/bench_seq", TESTDIR);
        char buf[4096];
        memset(buf, 'X', sizeof(buf));
        int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
        if (fd >= 0) {
            long long start = now_us();
            int total = 0;
            for (int i = 0; i < 256; i++) { // 1MB
                int n = write(fd, buf, sizeof(buf));
                if (n > 0) total += n;
            }
            close(fd);
            long long elapsed = now_us() - start;
            if (elapsed > 0) {
                long long mbps = (long long)total * 1000000 / elapsed / (1024*1024);
                msgf("BENCH seq_write: %d bytes in %lld us = %lld MB/s\n", total, elapsed, mbps);
            }
        }
    }

    // ── Test 10: Benchmark — sequential read throughput ──
    {
        snprintf(path, sizeof(path), "%s/bench_seq", TESTDIR);
        char buf[4096];
        int fd = open(path, O_RDONLY);
        if (fd >= 0) {
            long long start = now_us();
            int total = 0;
            int n;
            while ((n = read(fd, buf, sizeof(buf))) > 0) total += n;
            close(fd);
            long long elapsed = now_us() - start;
            if (elapsed > 0) {
                long long mbps = (long long)total * 1000000 / elapsed / (1024*1024);
                msgf("BENCH seq_read: %d bytes in %lld us = %lld MB/s\n", total, elapsed, mbps);
            }
        }
    }

    // ── Test 11: Check installed curl binary ──
    {
        struct stat st;
        const char *bins[] = {"/usr/bin/curl", "/sbin/apk", "/usr/bin/gcc", NULL};
        for (int i = 0; bins[i]; i++) {
            if (stat(bins[i], &st) == 0) {
                msgf("BINARY %s size=%ld mode=%o\n", bins[i], (long)st.st_size, st.st_mode);
                if (st.st_size == 0)
                    msgf("  ** ZERO-SIZE BINARY — ext4 write bug!\n");
            }
        }
    }

    // ── Test 12: Run /sbin/apk --version ──
    {
        char out[4096];
        int pipefd[2];
        pipe(pipefd);
        pid_t pid = fork();
        if (pid == 0) {
            close(pipefd[0]);
            dup2(pipefd[1], 1); dup2(pipefd[1], 2);
            close(pipefd[1]);
            execl("/sbin/apk", "apk", "--version", (char*)NULL);
            _exit(127);
        }
        close(pipefd[1]);
        int total = 0;
        while (total < (int)sizeof(out) - 1) {
            int n = read(pipefd[0], out + total, sizeof(out) - 1 - total);
            if (n <= 0) break;
            total += n;
        }
        out[total] = 0;
        close(pipefd[0]);
        int st;
        waitpid(pid, &st, 0);
        int rc = WIFEXITED(st) ? WEXITSTATUS(st) : -1;
        msgf("APK --version: exit=%d len=%d output='%.80s'\n", rc, total, out);
    }

    // ── Results ──
    int total = PASS + FAIL;
    msgf("TEST_END %d/%d\n", PASS, total);
    if (FAIL > 0) msgf("EXT4 TESTS: %d failure(s)\n", FAIL);
    else msg("ALL EXT4 TESTS PASSED\n");

    // Clean up
    char cmd[256];
    snprintf(cmd, sizeof(cmd), "rm -rf %s", TESTDIR);
    system(cmd);

    return FAIL > 0 ? 1 : 0;
}
