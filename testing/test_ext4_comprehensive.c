// Comprehensive ext4 + dynamic linking test suite for Kevlar.
// Statically linked with musl — runs without any dynamic libraries.
// Tests every file I/O mechanism, then tests dynamic binary execution.
//
// Build: musl-gcc -static -O2 -o test_ext4_comprehensive test_ext4_comprehensive.c
// Run: after pivot_root into Alpine rootfs, /usr/bin/test_ext4_comprehensive

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
#include <sys/uio.h>
#include <sys/sendfile.h>
#include <stdarg.h>
#include <time.h>
#include <dirent.h>

static int PASS = 0, FAIL = 0;
static void msg(const char *s) { write(2, s, strlen(s)); }
static void msgf(const char *fmt, ...) {
    char buf[512]; va_list ap;
    va_start(ap, fmt); int n = vsnprintf(buf, sizeof(buf), fmt, ap); va_end(ap);
    write(2, buf, n);
}
static void pass(const char *n) { msgf("TEST_PASS %s\n", n); PASS++; }
static void fail(const char *n, const char *r) { msgf("TEST_FAIL %s (%s)\n", n, r); FAIL++; }

static const char *TD = "/var/ext4test";

// Run command, capture output, return exit code
static int run(const char *const argv[], char *out, int sz) {
    int p[2]; pipe(p);
    pid_t pid = fork();
    if (pid == 0) {
        close(p[0]); dup2(p[1],1); dup2(p[1],2); close(p[1]);
        execv(argv[0], (char*const*)argv);
        dprintf(2, "exec %s: errno=%d\n", argv[0], errno);
        _exit(127);
    }
    close(p[1]);
    int t = 0;
    while (t < sz-1) {
        int n = read(p[0], out+t, sz-1-t);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) break;
        t += n;
    }
    out[t] = 0; close(p[0]);
    int st; waitpid(pid, &st, 0);
    if (WIFSIGNALED(st)) return -(WTERMSIG(st));
    return WIFEXITED(st) ? WEXITSTATUS(st) : -1;
}

static long long now_us(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (long long)ts.tv_sec * 1000000 + ts.tv_nsec / 1000;
}

// ── FILE I/O TESTS ─────────────────────────────────────────────────

static void test_basic_write(void) {
    char p[256]; snprintf(p, sizeof(p), "%s/t_write", TD);
    int fd = open(p, O_WRONLY|O_CREAT|O_TRUNC, 0644);
    if (fd < 0) { fail("write_open", strerror(errno)); return; }
    const char *d = "Hello ext4!\n";
    int n = write(fd, d, strlen(d));
    close(fd);
    // Read back
    fd = open(p, O_RDONLY);
    char buf[64]; int r = read(fd, buf, sizeof(buf)); close(fd);
    buf[r] = 0;
    if (n == (int)strlen(d) && strcmp(buf, d) == 0) pass("basic_write");
    else fail("basic_write", "content mismatch");
}

static void test_large_write(void) {
    char p[256]; snprintf(p, sizeof(p), "%s/t_large", TD);
    int fd = open(p, O_WRONLY|O_CREAT|O_TRUNC, 0644);
    char buf[4096]; memset(buf, 'A', sizeof(buf));
    int total = 0;
    for (int i = 0; i < 64; i++) total += write(fd, buf, sizeof(buf));
    close(fd);
    struct stat st; stat(p, &st);
    if (st.st_size == 262144) pass("large_write_256k");
    else { char r[32]; snprintf(r,32,"size=%ld",(long)st.st_size); fail("large_write_256k",r); }
}

static void test_writev(void) {
    char p[256]; snprintf(p, sizeof(p), "%s/t_writev", TD);
    int fd = open(p, O_WRONLY|O_CREAT|O_TRUNC, 0644);
    struct iovec iov[3] = {{"AAA",3},{"BBB",3},{"CCC\n",4}};
    int n = writev(fd, iov, 3); close(fd);
    // Read back
    fd = open(p, O_RDONLY); char buf[64]; int r = read(fd, buf, 63); close(fd);
    buf[r] = 0;
    if (n == 10 && strcmp(buf, "AAABBBCCC\n") == 0) pass("writev");
    else { char r2[32]; snprintf(r2,32,"n=%d",n); fail("writev",r2); }
}

static void test_pwrite_pread(void) {
    char p[256]; snprintf(p, sizeof(p), "%s/t_pwrite", TD);
    int fd = open(p, O_RDWR|O_CREAT|O_TRUNC, 0644);
    pwrite(fd, "AAAA", 4, 0);
    pwrite(fd, "BBBB", 4, 100);
    char buf[8];
    pread(fd, buf, 4, 0); buf[4] = 0;
    char buf2[8];
    pread(fd, buf2, 4, 100); buf2[4] = 0;
    close(fd);
    struct stat st; stat(p, &st);
    if (st.st_size == 104 && strcmp(buf,"AAAA")==0 && strcmp(buf2,"BBBB")==0) pass("pwrite_pread");
    else fail("pwrite_pread", "mismatch");
}

static void test_append(void) {
    char p[256]; snprintf(p, sizeof(p), "%s/t_append", TD);
    int fd = open(p, O_WRONLY|O_CREAT|O_TRUNC, 0644);
    write(fd, "first", 5); close(fd);
    fd = open(p, O_WRONLY|O_APPEND);
    write(fd, "second", 6); close(fd);
    fd = open(p, O_RDONLY); char buf[32]; int n = read(fd, buf, 31); close(fd);
    buf[n] = 0;
    if (strcmp(buf, "firstsecond") == 0) pass("append");
    else fail("append", buf);
}

static void test_ftruncate(void) {
    char p[256]; snprintf(p, sizeof(p), "%s/t_ftrunc", TD);
    int fd = open(p, O_WRONLY|O_CREAT|O_TRUNC, 0644);
    write(fd, "data", 4);
    ftruncate(fd, 8192);
    close(fd);
    struct stat st; stat(p, &st);
    // Read back: first 4 bytes should be "data", rest should be zeros
    fd = open(p, O_RDONLY);
    char buf[16]; read(fd, buf, 16); close(fd);
    if (st.st_size == 8192 && memcmp(buf, "data", 4) == 0 && buf[4] == 0) pass("ftruncate");
    else { char r[32]; snprintf(r,32,"size=%ld",(long)st.st_size); fail("ftruncate",r); }
}

static void test_mmap_shared_write(void) {
    char p[256]; snprintf(p, sizeof(p), "%s/t_mmap_shared", TD);
    int fd = open(p, O_RDWR|O_CREAT|O_TRUNC, 0644);
    ftruncate(fd, 4096);
    void *m = mmap(NULL, 4096, PROT_READ|PROT_WRITE, MAP_SHARED, fd, 0);
    if (m == MAP_FAILED) { fail("mmap_shared_write", "mmap failed"); close(fd); return; }
    memcpy(m, "MMAP_SHARED_OK!", 15);
    msync(m, 4096, MS_SYNC);
    munmap(m, 4096);
    close(fd);
    // Read back from file
    fd = open(p, O_RDONLY); char buf[32]; int n = read(fd, buf, 31); close(fd);
    buf[n > 0 ? n : 0] = 0;
    if (strncmp(buf, "MMAP_SHARED_OK!", 15) == 0) pass("mmap_shared_write");
    else { char r[64]; snprintf(r, 64, "got='%.20s'", buf); fail("mmap_shared_write", r); }
}

static void test_mmap_shared_unaligned(void) {
    // Test mmap writeback for a file that's NOT page-aligned in size
    char p[256]; snprintf(p, sizeof(p), "%s/t_mmap_unaligned", TD);
    int fd = open(p, O_RDWR|O_CREAT|O_TRUNC, 0644);
    // Write 5000 bytes (not page-aligned: 1 full page + 904 bytes)
    char data[5000];
    memset(data, 'X', sizeof(data));
    memcpy(data, "HEADER", 6);
    memcpy(data + 4990, "TAIL!", 5);
    write(fd, data, sizeof(data));
    close(fd);
    // Now mmap the file, modify it, munmap
    fd = open(p, O_RDWR);
    void *m = mmap(NULL, 8192, PROT_READ|PROT_WRITE, MAP_SHARED, fd, 0);
    if (m == MAP_FAILED) { fail("mmap_unaligned", "mmap failed"); close(fd); return; }
    memcpy(m, "MAPPED", 6); // Overwrite header
    munmap(m, 8192);
    close(fd);
    // Read back and verify: file should still be 5000 bytes
    struct stat st; stat(p, &st);
    fd = open(p, O_RDONLY);
    char rbuf[5000]; int n = read(fd, rbuf, sizeof(rbuf)); close(fd);
    int ok = (st.st_size == 5000) && (n == 5000) &&
             (memcmp(rbuf, "MAPPED", 6) == 0) &&
             (memcmp(rbuf + 4990, "TAIL!", 5) == 0);
    if (ok) pass("mmap_unaligned");
    else { char r[64]; snprintf(r,64,"size=%ld n=%d",(long)st.st_size,n); fail("mmap_unaligned",r); }
}

static void test_sendfile(void) {
    char src[256], dst[256];
    snprintf(src, sizeof(src), "%s/t_sf_src", TD);
    snprintf(dst, sizeof(dst), "%s/t_sf_dst", TD);
    int sfd = open(src, O_WRONLY|O_CREAT|O_TRUNC, 0644);
    char data[1024]; memset(data, 'S', sizeof(data));
    memcpy(data, "SENDFILE", 8);
    write(sfd, data, sizeof(data)); close(sfd);
    sfd = open(src, O_RDONLY);
    int dfd = open(dst, O_WRONLY|O_CREAT|O_TRUNC, 0644);
    off_t off = 0;
    ssize_t copied = sendfile(dfd, sfd, &off, 1024);
    close(sfd); close(dfd);
    // Read back
    dfd = open(dst, O_RDONLY); char buf[16]; read(dfd, buf, 16); close(dfd);
    if (copied == 1024 && memcmp(buf, "SENDFILE", 8) == 0) pass("sendfile");
    else { char r[32]; snprintf(r,32,"copied=%zd",copied); fail("sendfile",r); }
}

static void test_mkdir_readdir(void) {
    char p[256]; snprintf(p, sizeof(p), "%s/t_dir", TD);
    mkdir(p, 0755);
    char f1[256]; snprintf(f1, sizeof(f1), "%s/file1", p);
    char f2[256]; snprintf(f2, sizeof(f2), "%s/file2", p);
    int fd = open(f1, O_WRONLY|O_CREAT, 0644); write(fd, "a", 1); close(fd);
    fd = open(f2, O_WRONLY|O_CREAT, 0644); write(fd, "b", 1); close(fd);
    DIR *d = opendir(p);
    int count = 0;
    if (d) {
        struct dirent *ent;
        while ((ent = readdir(d))) {
            if (ent->d_name[0] != '.') count++;
        }
        closedir(d);
    }
    if (count == 2) pass("mkdir_readdir");
    else { char r[32]; snprintf(r,32,"count=%d",count); fail("mkdir_readdir",r); }
}

static void test_rename(void) {
    char src[256], dst[256];
    snprintf(src, sizeof(src), "%s/t_rename_src", TD);
    snprintf(dst, sizeof(dst), "%s/t_rename_dst", TD);
    int fd = open(src, O_WRONLY|O_CREAT|O_TRUNC, 0644);
    write(fd, "moved", 5); close(fd);
    rename(src, dst);
    struct stat st;
    int src_gone = (stat(src, &st) != 0);
    fd = open(dst, O_RDONLY); char buf[16]; int n = read(fd, buf, 15); close(fd);
    buf[n > 0 ? n : 0] = 0;
    if (src_gone && strcmp(buf, "moved") == 0) pass("rename");
    else fail("rename", "mismatch");
}

static void test_unlink(void) {
    char p[256]; snprintf(p, sizeof(p), "%s/t_unlink", TD);
    int fd = open(p, O_WRONLY|O_CREAT|O_TRUNC, 0644);
    write(fd, "x", 1); close(fd);
    unlink(p);
    struct stat st;
    if (stat(p, &st) != 0) pass("unlink");
    else fail("unlink", "file still exists");
}

static void test_chmod(void) {
    char p[256]; snprintf(p, sizeof(p), "%s/t_chmod", TD);
    int fd = open(p, O_WRONLY|O_CREAT|O_TRUNC, 0644); close(fd);
    chmod(p, 0755);
    struct stat st; stat(p, &st);
    if ((st.st_mode & 0777) == 0755) pass("chmod");
    else { char r[32]; snprintf(r,32,"mode=%o",st.st_mode&0777); fail("chmod",r); }
}

static void test_symlink(void) {
    char target[256], link[256];
    snprintf(target, sizeof(target), "%s/t_symlink_target", TD);
    snprintf(link, sizeof(link), "%s/t_symlink_link", TD);
    int fd = open(target, O_WRONLY|O_CREAT|O_TRUNC, 0644);
    write(fd, "symlink_data", 12); close(fd);
    symlink(target, link);
    fd = open(link, O_RDONLY);
    char buf[32]; int n = read(fd, buf, 31); close(fd);
    buf[n > 0 ? n : 0] = 0;
    if (strcmp(buf, "symlink_data") == 0) pass("symlink");
    else fail("symlink", buf);
}

// ── DYNAMIC LINKING TESTS ──────────────────────────────────────────

static void test_dynamic_exec(const char *name, const char *path, const char *expected) {
    char out[4096];
    const char *argv[] = {path, "--version", NULL};
    int rc = run(argv, out, sizeof(out));
    if (rc == 0 && strlen(out) > 0) {
        if (expected == NULL || strstr(out, expected)) pass(name);
        else { char r[128]; snprintf(r,128,"unexpected: %.60s",out); fail(name,r); }
    } else if (rc == 127) {
        fail(name, "ENOEXEC or not found");
    } else {
        char r[64]; snprintf(r,64,"exit=%d len=%d",rc,(int)strlen(out)); fail(name,r);
    }
}

// ── BENCHMARKS ─────────────────────────────────────────────────────

static void bench_seq_write(void) {
    char p[256]; snprintf(p, sizeof(p), "%s/bench_write", TD);
    char buf[4096]; memset(buf, 'W', sizeof(buf));
    int fd = open(p, O_WRONLY|O_CREAT|O_TRUNC, 0644);
    long long start = now_us();
    int total = 0;
    for (int i = 0; i < 256; i++) total += write(fd, buf, sizeof(buf));
    close(fd);
    long long elapsed = now_us() - start;
    long long kbps = elapsed > 0 ? (long long)total * 1000 / elapsed : 0;
    msgf("BENCH seq_write_1mb: %d bytes, %lld us, %lld KB/s\n", total, elapsed, kbps);
}

static void bench_seq_read(void) {
    char p[256]; snprintf(p, sizeof(p), "%s/bench_write", TD);
    char buf[4096];
    int fd = open(p, O_RDONLY);
    long long start = now_us();
    int total = 0, n;
    while ((n = read(fd, buf, sizeof(buf))) > 0) total += n;
    close(fd);
    long long elapsed = now_us() - start;
    long long kbps = elapsed > 0 ? (long long)total * 1000 / elapsed : 0;
    msgf("BENCH seq_read_1mb: %d bytes, %lld us, %lld KB/s\n", total, elapsed, kbps);
}

static void bench_create_delete(void) {
    long long start = now_us();
    int count = 100;
    for (int i = 0; i < count; i++) {
        char p[256]; snprintf(p, sizeof(p), "%s/bench_%04d", TD, i);
        int fd = open(p, O_WRONLY|O_CREAT|O_TRUNC, 0644);
        write(fd, "x", 1); close(fd);
    }
    for (int i = 0; i < count; i++) {
        char p[256]; snprintf(p, sizeof(p), "%s/bench_%04d", TD, i);
        unlink(p);
    }
    long long elapsed = now_us() - start;
    msgf("BENCH create_delete_%d: %lld us, %lld us/op\n", count, elapsed, elapsed/(count*2));
}

int main(int argc, char **argv) {
    msg("=== Kevlar ext4 + dynamic linking test suite ===\n");
    mkdir(TD, 0755);

    // File I/O tests
    test_basic_write();
    test_large_write();
    test_writev();
    test_pwrite_pread();
    test_append();
    test_ftruncate();
    test_mmap_shared_write();
    test_mmap_shared_unaligned();
    test_sendfile();
    test_mkdir_readdir();
    test_rename();
    test_unlink();
    test_chmod();
    test_symlink();

    // Dynamic linking tests (programs from Alpine packages)
    test_dynamic_exec("busybox_version", "/bin/busybox", "BusyBox");
    test_dynamic_exec("curl_version", "/usr/bin/curl", "curl");
    test_dynamic_exec("apk_version", "/sbin/apk", "apk-tools");

    // Benchmarks
    bench_seq_write();
    bench_seq_read();
    bench_create_delete();

    // Summary
    int total = PASS + FAIL;
    msgf("\n=== RESULTS: %d/%d passed ===\n", PASS, total);
    if (FAIL > 0) msgf("FAILURES: %d\n", FAIL);
    msgf("TEST_END %d/%d\n", PASS, total);

    // Cleanup
    char cmd[256]; snprintf(cmd, sizeof(cmd), "rm -rf %s", TD);
    system(cmd);

    return FAIL > 0 ? 1 : 0;
}
