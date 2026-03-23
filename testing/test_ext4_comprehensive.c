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
    if ((st.st_mode & 0777) == 0755) pass("chmod_755");
    else { char r[32]; snprintf(r,32,"mode=%o",st.st_mode&0777); fail("chmod_755",r); }

    // Also test setting to 0600, 0444, and back
    chmod(p, 0600);
    stat(p, &st);
    if ((st.st_mode & 0777) == 0600) pass("chmod_600");
    else { char r[32]; snprintf(r,32,"mode=%o",st.st_mode&0777); fail("chmod_600",r); }

    chmod(p, 0444);
    stat(p, &st);
    if ((st.st_mode & 0777) == 0444) pass("chmod_444");
    else { char r[32]; snprintf(r,32,"mode=%o",st.st_mode&0777); fail("chmod_444",r); }
}

static void test_hardlink(void) {
    char src[256], dst[256];
    snprintf(src, sizeof(src), "%s/t_hardlink_src", TD);
    snprintf(dst, sizeof(dst), "%s/t_hardlink_dst", TD);
    int fd = open(src, O_WRONLY|O_CREAT|O_TRUNC, 0644);
    write(fd, "hardlink", 8); close(fd);
    link(src, dst);
    struct stat st1, st2;
    stat(src, &st1);
    stat(dst, &st2);
    // Hard links share the same inode
    if (st1.st_ino == st2.st_ino && st1.st_nlink == 2)
        pass("hardlink");
    else {
        char r[64]; snprintf(r,64,"ino1=%ld ino2=%ld nlink=%ld",
                             (long)st1.st_ino, (long)st2.st_ino, (long)st1.st_nlink);
        fail("hardlink", r);
    }
}

static void test_sparse_file(void) {
    char p[256]; snprintf(p, sizeof(p), "%s/t_sparse", TD);
    int fd = open(p, O_WRONLY|O_CREAT|O_TRUNC, 0644);
    // Write 8 bytes at offset 1MB to create a sparse file
    pwrite(fd, "SPARSE!!", 8, 1024*1024);
    close(fd);
    struct stat st; stat(p, &st);
    // File should be ~1MB+8 in size
    if (st.st_size == 1024*1024 + 8) pass("sparse_file");
    else { char r[32]; snprintf(r,32,"size=%ld",(long)st.st_size); fail("sparse_file",r); }
    // Read back from the sparse region (should be zeros)
    fd = open(p, O_RDONLY);
    char buf[8];
    pread(fd, buf, 8, 0);
    int zeros_ok = (buf[0] == 0 && buf[7] == 0);
    pread(fd, buf, 8, 1024*1024);
    int data_ok = (memcmp(buf, "SPARSE!!", 8) == 0);
    close(fd);
    if (zeros_ok && data_ok) pass("sparse_readback");
    else fail("sparse_readback", "content mismatch");
}

static void test_many_files(void) {
    char dir[256]; snprintf(dir, sizeof(dir), "%s/t_manyfiles", TD);
    mkdir(dir, 0755);
    int ok = 1;
    for (int i = 0; i < 50; i++) {
        char p[300]; snprintf(p, sizeof(p), "%s/f%04d", dir, i);
        int fd = open(p, O_WRONLY|O_CREAT|O_TRUNC, 0644);
        if (fd < 0) { ok = 0; break; }
        char data[32]; snprintf(data, sizeof(data), "file %d\n", i);
        write(fd, data, strlen(data));
        close(fd);
    }
    // Count via readdir
    DIR *d = opendir(dir);
    int count = 0;
    if (d) {
        struct dirent *ent;
        while ((ent = readdir(d))) if (ent->d_name[0] != '.') count++;
        closedir(d);
    }
    if (ok && count == 50) pass("many_files_50");
    else { char r[32]; snprintf(r,32,"ok=%d count=%d",ok,count); fail("many_files_50",r); }
}

static void test_large_readwrite(void) {
    // Write 4MB file and verify content
    char p[256]; snprintf(p, sizeof(p), "%s/t_large4m", TD);
    int fd = open(p, O_WRONLY|O_CREAT|O_TRUNC, 0644);
    char buf[4096];
    for (int i = 0; i < 4096; i++) buf[i] = (i * 7 + 13) & 0xFF;
    int total = 0;
    for (int i = 0; i < 1024; i++) { // 4MB
        int n = write(fd, buf, sizeof(buf));
        if (n > 0) total += n;
    }
    close(fd);
    struct stat st; stat(p, &st);
    if (st.st_size != 4*1024*1024) {
        char r[32]; snprintf(r,32,"size=%ld",(long)st.st_size);
        fail("large_4mb_write", r);
        return;
    }
    // Read back and verify
    fd = open(p, O_RDONLY);
    int ok = 1;
    for (int i = 0; i < 1024; i++) {
        char rbuf[4096];
        int n = read(fd, rbuf, sizeof(rbuf));
        if (n != 4096 || memcmp(rbuf, buf, 4096) != 0) { ok = 0; break; }
    }
    close(fd);
    if (ok) pass("large_4mb_readback");
    else fail("large_4mb_readback", "content mismatch");
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
    // First check the file exists and has ELF magic
    struct stat fst;
    if (stat(path, &fst) != 0) {
        char r[64]; snprintf(r, 64, "stat failed: errno=%d", errno);
        fail(name, r);
        return;
    }
    int fd = open(path, O_RDONLY);
    if (fd >= 0) {
        unsigned char magic[4];
        read(fd, magic, 4);
        close(fd);
        if (magic[0] != 0x7f || magic[1] != 'E' || magic[2] != 'L' || magic[3] != 'F') {
            char r[64]; snprintf(r, 64, "not ELF: %02x%02x%02x%02x size=%ld",
                                 magic[0], magic[1], magic[2], magic[3], (long)fst.st_size);
            fail(name, r);
            return;
        }
    }
    msgf("  %s: ELF ok, size=%ld\n", path, (long)fst.st_size);

    char out[4096];
    const char *argv[] = {path, "--version", NULL};
    int rc = run(argv, out, sizeof(out));
    if (rc == 0 && strlen(out) > 0) {
        if (expected == NULL || strstr(out, expected)) pass(name);
        else { char r[128]; snprintf(r,128,"unexpected: %.60s",out); fail(name,r); }
    } else if (rc == 127) {
        char r[64]; snprintf(r, 64, "ENOEXEC/not found, output='%.40s'", out);
        fail(name, r);
    } else {
        char r[96]; snprintf(r, 96, "exit=%d len=%d output='%.40s'", rc, (int)strlen(out), out);
        fail(name, r);
    }
}

// ── BENCHMARKS ─────────────────────────────────────────────────────

static void bench_seq_write(void) {
    // Test with multiple buffer sizes to find the optimal
    int sizes[] = {4096, 32768, 131072, 0};
    for (int si = 0; sizes[si]; si++) {
        int bufsz = sizes[si];
        char *buf = malloc(bufsz);
        memset(buf, 'W', bufsz);
        char p[256]; snprintf(p, sizeof(p), "%s/bench_w_%d", TD, bufsz);
        int fd = open(p, O_WRONLY|O_CREAT|O_TRUNC, 0644);
        int target = 4*1024*1024; // 4MB
        long long start = now_us();
        int total = 0;
        while (total < target) {
            int n = write(fd, buf, bufsz);
            if (n <= 0) break;
            total += n;
        }
        close(fd);
        long long elapsed = now_us() - start;
        long long kbps = elapsed > 0 ? (long long)total * 1000 / elapsed : 0;
        msgf("BENCH seq_write_%dk_buf%dk: %d bytes, %lld us, %lld KB/s\n",
             total/1024, bufsz/1024, total, elapsed, kbps);
        free(buf);
        unlink(p);
    }
}

static void bench_seq_read(void) {
    // Write 4MB first, then read with different buffer sizes
    char p[256]; snprintf(p, sizeof(p), "%s/bench_read", TD);
    {
        int fd = open(p, O_WRONLY|O_CREAT|O_TRUNC, 0644);
        char buf[32768]; memset(buf, 'R', sizeof(buf));
        for (int i = 0; i < 128; i++) write(fd, buf, sizeof(buf));
        close(fd);
    }
    int sizes[] = {4096, 32768, 131072, 0};
    for (int si = 0; sizes[si]; si++) {
        int bufsz = sizes[si];
        char *buf = malloc(bufsz);
        int fd = open(p, O_RDONLY);
        long long start = now_us();
        int total = 0, n;
        while ((n = read(fd, buf, bufsz)) > 0) total += n;
        close(fd);
        long long elapsed = now_us() - start;
        long long kbps = elapsed > 0 ? (long long)total * 1000 / elapsed : 0;
        msgf("BENCH seq_read_%dk_buf%dk: %d bytes, %lld us, %lld KB/s\n",
             total/1024, bufsz/1024, total, elapsed, kbps);
        free(buf);
    }
    unlink(p);
}

static void bench_create_delete(void) {
    // Measure create and delete separately
    int count = 100;
    long long start = now_us();
    for (int i = 0; i < count; i++) {
        char p[256]; snprintf(p, sizeof(p), "%s/bench_%04d", TD, i);
        int fd = open(p, O_WRONLY|O_CREAT|O_TRUNC, 0644);
        write(fd, "x", 1); close(fd);
    }
    long long create_elapsed = now_us() - start;

    start = now_us();
    for (int i = 0; i < count; i++) {
        char p[256]; snprintf(p, sizeof(p), "%s/bench_%04d", TD, i);
        unlink(p);
    }
    long long delete_elapsed = now_us() - start;

    msgf("BENCH create_%d: %lld us, %lld us/op\n", count, create_elapsed, create_elapsed/count);
    msgf("BENCH delete_%d: %lld us, %lld us/op\n", count, delete_elapsed, delete_elapsed/count);

    // Measure just open+close (no write) to isolate overhead
    start = now_us();
    for (int i = 0; i < count; i++) {
        char p[256]; snprintf(p, sizeof(p), "%s/bench_%04d", TD, i);
        int fd = open(p, O_WRONLY|O_CREAT|O_TRUNC, 0644);
        close(fd);
    }
    long long openclose_elapsed = now_us() - start;
    msgf("BENCH open_close_%d: %lld us, %lld us/op\n", count, openclose_elapsed, openclose_elapsed/count);
    for (int i = 0; i < count; i++) {
        char p[256]; snprintf(p, sizeof(p), "%s/bench_%04d", TD, i);
        unlink(p);
    }

    // Measure stat() on the same file 1000 times (tests read cache)
    {
        char p[256]; snprintf(p, sizeof(p), "%s/bench_stat", TD);
        int fd = open(p, O_WRONLY|O_CREAT|O_TRUNC, 0644);
        write(fd, "x", 1); close(fd);
        struct stat st;
        start = now_us();
        for (int i = 0; i < 1000; i++) stat(p, &st);
        long long stat_elapsed = now_us() - start;
        msgf("BENCH stat_1000: %lld us, %lld ns/op\n", stat_elapsed, stat_elapsed * 1000 / 1000);
        unlink(p);
    }

    // Measure deep path resolution: /var/ext4test/d1/d2/d3/d4/file
    {
        char base[256]; snprintf(base, sizeof(base), "%s/d1", TD);
        mkdir(base, 0755);
        char d2[256]; snprintf(d2, sizeof(d2), "%s/d2", base);
        mkdir(d2, 0755);
        char d3[256]; snprintf(d3, sizeof(d3), "%s/d3", d2);
        mkdir(d3, 0755);
        char d4[256]; snprintf(d4, sizeof(d4), "%s/d4", d3);
        mkdir(d4, 0755);
        char deep[256]; snprintf(deep, sizeof(deep), "%s/file", d4);
        int fd = open(deep, O_WRONLY|O_CREAT, 0644);
        write(fd, "x", 1); close(fd);

        struct stat st;
        start = now_us();
        for (int i = 0; i < 100; i++) stat(deep, &st);
        long long deep_elapsed = now_us() - start;
        msgf("BENCH deep_stat_100: %lld us, %lld us/op (5-component path)\n",
             deep_elapsed, deep_elapsed / 100);
    }
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
    test_hardlink();
    test_sparse_file();
    test_many_files();
    test_large_readwrite();

    // ── Dynamic linking tests ──────────────────────────────────────
    // BusyBox (links: just musl) — known to work
    {
        char out[4096];
        const char *argv[] = {"/bin/busybox", "--help", NULL};
        int rc = run(argv, out, sizeof(out));
        if (strstr(out, "BusyBox")) pass("dyn_busybox");
        else { char r[64]; snprintf(r,64,"exit=%d len=%d",rc,(int)strlen(out)); fail("dyn_busybox",r); }
    }

    // Incremental library loading tests: write tiny test programs to /tmp,
    // compile with gcc (if available) or just test existing binaries.
    // Since gcc may not work, test existing Alpine binaries that link
    // different numbers of libraries:
    //
    // openrc (links: libeinfo, librc, musl) — works for OpenRC boot
    // apk (links: libapk, libcrypto, libssl, libz, musl) — fails
    // curl (links: libcurl, libcrypto, libssl, libz, + more, musl) — fails

    // Test: /sbin/openrc --help (links libeinfo + librc)
    {
        char out[4096];
        const char *argv[] = {"/sbin/openrc", "--help", NULL};
        int rc = run(argv, out, sizeof(out));
        // openrc --help exits 1 but produces output
        if (strlen(out) > 0) pass("dyn_openrc");
        else { char r[64]; snprintf(r,64,"exit=%d len=%d",rc,(int)strlen(out)); fail("dyn_openrc",r); }
    }

    // Test: /usr/bin/file --version (if installed — links libmagic + libz)
    {
        struct stat fst;
        if (stat("/usr/bin/file", &fst) == 0 && fst.st_size > 0) {
            test_dynamic_exec("dyn_file", "/usr/bin/file", NULL);
        } else {
            msgf("  SKIP dyn_file (not installed)\n");
        }
    }

    // Test existing multi-lib binaries
    test_dynamic_exec("dyn_curl", "/usr/bin/curl", "curl");
    test_dynamic_exec("dyn_apk", "/sbin/apk", "apk-tools");

    // Test: run debug-curl to trace exactly where curl fails
    {
        struct stat fst2;
        if (stat("/usr/bin/curl-debug", &fst2) == 0) {
            char out[4096];
            const char *argv[] = {"/usr/bin/curl-debug", NULL};
            int rc = run(argv, out, sizeof(out));
            msgf("CURL_DEBUG: exit=%d output='%.300s'\n", rc, out);
        } else {
            msgf("CURL_DEBUG: binary not found\n");
        }
    }

    // ── Test: isolate libcrypto as the cause ──
    // LD_PRELOAD libcrypto into busybox (which normally works).
    // If this fails, libcrypto's constructor is the problem.
    {
        char out[4096];
        // Test A: busybox echo without libcrypto (should work)
        const char *argv_a[] = {"/bin/busybox", "echo", "NO_CRYPTO", NULL};
        int rc_a = run(argv_a, out, sizeof(out));

        // Test B: busybox echo WITH libcrypto preloaded
        // We can't set env in run(), so use sh -c with env
        const char *argv_b[] = {"/bin/sh", "-c",
            "LD_PRELOAD=/lib/libcrypto.so.3 /bin/busybox echo WITH_CRYPTO", NULL};
        char out_b[4096];
        int rc_b = run(argv_b, out_b, sizeof(out_b));

        msgf("CRYPTO_TEST: without=%d('%s') with=%d('%s')\n",
             rc_a, out[0] ? out : "(empty)",
             rc_b, out_b[0] ? out_b : "(empty)");

        if (strstr(out, "NO_CRYPTO") && strstr(out_b, "WITH_CRYPTO"))
            pass("libcrypto_preload");
        else if (strstr(out, "NO_CRYPTO") && !strstr(out_b, "WITH_CRYPTO"))
            fail("libcrypto_preload", "libcrypto constructor breaks programs");
        else
            fail("libcrypto_preload", "unexpected");

        // Test C: preload ALL of curl's dependencies
        const char *argv_c[] = {"/bin/sh", "-c",
            "LD_PRELOAD='/usr/lib/libcurl.so.4 /lib/libcrypto.so.3 /lib/libssl.so.3 /lib/libz.so.1' "
            "/bin/busybox echo ALL_LIBS", NULL};
        char out_c[4096];
        int rc_c = run(argv_c, out_c, sizeof(out_c));
        msgf("ALL_LIBS_TEST: exit=%d output='%.40s'\n", rc_c, out_c);
        if (strstr(out_c, "ALL_LIBS")) pass("all_libs_preload");
        else fail("all_libs_preload", out_c[0] ? out_c : "(empty)");

        // Test D: run curl --version and capture result
        const char *argv_d[] = {"/bin/sh", "-c",
            "/usr/bin/curl --version 2>&1; echo CURL_EXIT=$?", NULL};
        char out_d[4096];
        int rc_d = run(argv_d, out_d, sizeof(out_d));
        msgf("CURL_SH_TEST: exit=%d output='%.100s'\n", rc_d, out_d);

        // Test E: test individual library constructors
        {
            const char *libs[] = {
                "/lib/libapk.so.2.14.0",
                "/usr/lib/libcurl.so.4",
                "/usr/lib/libbrotlidec.so.1",
                "/usr/lib/libcares.so.2",
                "/usr/lib/libidn2.so.0",
                "/usr/lib/libnghttp2.so.14",
                "/usr/lib/libpsl.so.5",
                NULL
            };
            for (int i = 0; libs[i]; i++) {
                char cmd[256];
                char out_e[256];
                snprintf(cmd, sizeof(cmd), "LD_PRELOAD=%s /bin/busybox echo LIB_OK", libs[i]);
                const char *argv_e[] = {"/bin/sh", "-c", cmd, NULL};
                int rc_e = run(argv_e, out_e, sizeof(out_e));
                if (strstr(out_e, "LIB_OK"))
                    msgf("LIB_CTOR %s: OK\n", libs[i]);
                else
                    msgf("LIB_CTOR %s: FAIL exit=%d output='%.40s'\n", libs[i], rc_e, out_e);
            }
        }

        // Test F: verify installed curl matches the REAL package binary
        // Real curl 8.14.1-r2: 256216 bytes, byte sum = 23686133
        {
            int fd = open("/usr/bin/curl", O_RDONLY);
            if (fd >= 0) {
                unsigned char buf[4096];
                unsigned long sum = 0;
                int total = 0, n;
                while ((n = read(fd, buf, sizeof(buf))) > 0 && total < 256216) {
                    int use = (total + n > 256216) ? (256216 - total) : n;
                    for (int j = 0; j < use; j++) sum += buf[j];
                    total += use;
                }
                close(fd);
                msgf("VERIFY curl: bytes_checked=%d sum=%lu expected=23686133 match=%s\n",
                     total, sum, sum == 23686133 ? "YES" : "NO");
                if (sum == 23686133) pass("curl_integrity");
                else { char r[64]; snprintf(r,64,"sum=%lu",sum); fail("curl_integrity",r); }
            } else {
                fail("curl_integrity", "can't open");
            }
        }

        // Test G: check first 16KB checksum
        // Compute a simple checksum of the first 16KB of the installed curl
        {
            int fd = open("/usr/bin/curl", O_RDONLY);
            if (fd >= 0) {
                unsigned char buf[4096];
                unsigned long sum = 0;
                int total_read = 0;
                // Checksum first 16KB
                for (int i = 0; i < 4 && total_read < 16384; i++) {
                    int n = read(fd, buf, sizeof(buf));
                    if (n <= 0) break;
                    for (int j = 0; j < n; j++) sum += buf[j];
                    total_read += n;
                }
                // Also read bytes around the section header area
                struct stat st; fstat(fd, &st);
                // Read last 4KB
                if (st.st_size > 4096) {
                    lseek(fd, st.st_size - 4096, SEEK_SET);
                    int n = read(fd, buf, 4096);
                    unsigned long tail_sum = 0;
                    for (int j = 0; j < n; j++) tail_sum += buf[j];
                    msgf("DETAIL curl_tail: last_4k_sum=%lu last_byte=%d\n",
                         tail_sum, n > 0 ? buf[n-1] : -1);
                    // Check if tail has all zeros (padding)
                    int zeros = 0;
                    for (int j = n - 1; j >= 0 && buf[j] == 0; j--) zeros++;
                    msgf("DETAIL curl_tail: trailing_zeros=%d\n", zeros);
                }
                close(fd);
                msgf("DETAIL curl_checksum: first_16k_sum=%lu bytes=%d\n", sum, total_read);
            }
        }
    }

    // ── Detailed binary analysis ──
    // Compare installed curl to expected size and check for ELF validity
    {
        struct stat st;
        if (stat("/usr/bin/curl", &st) == 0) {
            // Real Alpine curl 8.14.1-r2 is 256216 bytes
            msgf("DETAIL curl: size=%ld expected=256216 diff=%ld\n",
                 (long)st.st_size, (long)st.st_size - 256216);
            // Read first 64 bytes (ELF header) and check key fields
            int fd = open("/usr/bin/curl", O_RDONLY);
            if (fd >= 0) {
                unsigned char eh[64];
                read(fd, eh, 64);
                unsigned short e_type = *(unsigned short*)(eh + 16);
                unsigned short e_machine = *(unsigned short*)(eh + 18);
                unsigned long e_entry = *(unsigned long*)(eh + 24);
                unsigned long e_phoff = *(unsigned long*)(eh + 32);
                unsigned short e_phnum = *(unsigned short*)(eh + 56);
                unsigned long e_shoff = *(unsigned long*)(eh + 40);
                unsigned short e_shnum = *(unsigned short*)(eh + 60);
                msgf("DETAIL curl ELF: type=%d machine=%d entry=%lx phoff=%ld phnum=%d shoff=%ld shnum=%d\n",
                     e_type, e_machine, e_entry, (long)e_phoff, e_phnum, (long)e_shoff, e_shnum);
                // Check if section headers are within file
                unsigned long sh_end = e_shoff + e_shnum * (*(unsigned short*)(eh + 58));
                msgf("DETAIL curl: sh_end=%ld file_size=%ld sh_within=%s\n",
                     (long)sh_end, (long)st.st_size,
                     sh_end <= (unsigned long)st.st_size ? "YES" : "NO");
                close(fd);
            }
        }
    }

    // ── Detailed library loading probe ──────────────────────────────
    // For each library that curl needs, check if it can be dlopen'd
    // by writing a tiny test and running it. Since we can't compile,
    // we'll check another way: run ldd-like analysis by reading the
    // dynamic section of each library and checking its NEEDED deps.
    {
        msg("\n=== Library dependency chain ===\n");
        const char *libs[] = {
            "/usr/bin/curl",
            "/usr/lib/libcurl.so.4",
            "/usr/lib/libcrypto.so.3",
            "/usr/lib/libssl.so.3",
            "/lib/libapk.so.2.14.0",
            NULL
        };
        for (int i = 0; libs[i]; i++) {
            // Read the ELF header to find PT_DYNAMIC
            int fd = open(libs[i], O_RDONLY);
            if (fd < 0) { msgf("  %s: not found\n", libs[i]); continue; }
            unsigned char ehdr[64];
            if (read(fd, ehdr, 64) != 64 || ehdr[0] != 0x7f) {
                close(fd); msgf("  %s: not ELF\n", libs[i]); continue;
            }
            unsigned long phoff = *(unsigned long*)(ehdr + 32);
            int phnum = *(unsigned short*)(ehdr + 56);
            int phentsz = *(unsigned short*)(ehdr + 54);

            // Find PT_DYNAMIC (type=2)
            unsigned long dyn_off = 0, dyn_sz = 0;
            for (int j = 0; j < phnum; j++) {
                unsigned char ph[56];
                lseek(fd, phoff + j * phentsz, SEEK_SET);
                if (read(fd, ph, 56) < 56) break;
                if (*(unsigned int*)ph == 2) { // PT_DYNAMIC
                    dyn_off = *(unsigned long*)(ph + 8);
                    dyn_sz = *(unsigned long*)(ph + 32);
                    break;
                }
            }

            // Find PT_LOAD for .dynstr (type=1, containing the string table)
            // Actually just read DT_NEEDED entries from .dynamic
            if (dyn_off == 0) { close(fd); continue; }

            // Find strtab offset from dynamic section
            unsigned long strtab_off = 0;
            unsigned char dynbuf[4096];
            lseek(fd, dyn_off, SEEK_SET);
            int dsz = read(fd, dynbuf, sizeof(dynbuf) < dyn_sz ? sizeof(dynbuf) : dyn_sz);

            // Parse DT entries (each is 16 bytes: tag + val)
            unsigned long needed[16];
            int needed_count = 0;
            for (int off = 0; off + 16 <= dsz; off += 16) {
                long tag = *(long*)(dynbuf + off);
                unsigned long val = *(unsigned long*)(dynbuf + off + 8);
                if (tag == 5) strtab_off = val; // DT_STRTAB (vaddr)
                if (tag == 1 && needed_count < 16) // DT_NEEDED
                    needed[needed_count++] = val;
                if (tag == 0) break; // DT_NULL
            }

            // strtab_off is a virtual address — we need the file offset
            // Find the PT_LOAD that contains strtab_off
            unsigned long strtab_file_off = 0;
            for (int j = 0; j < phnum; j++) {
                unsigned char ph[56];
                lseek(fd, phoff + j * phentsz, SEEK_SET);
                if (read(fd, ph, 56) < 56) break;
                if (*(unsigned int*)ph == 1) { // PT_LOAD
                    unsigned long p_vaddr = *(unsigned long*)(ph + 16);
                    unsigned long p_offset = *(unsigned long*)(ph + 8);
                    unsigned long p_filesz = *(unsigned long*)(ph + 32);
                    if (strtab_off >= p_vaddr && strtab_off < p_vaddr + p_filesz) {
                        strtab_file_off = p_offset + (strtab_off - p_vaddr);
                        break;
                    }
                }
            }

            msgf("  %s needs:", libs[i]);
            if (strtab_file_off > 0) {
                char strtab[4096];
                lseek(fd, strtab_file_off, SEEK_SET);
                int stsz = read(fd, strtab, sizeof(strtab));
                for (int k = 0; k < needed_count; k++) {
                    if (needed[k] < (unsigned long)stsz)
                        msgf(" %s", strtab + needed[k]);
                }
            }
            msgf("\n");
            close(fd);
        }
    }

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
