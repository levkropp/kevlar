// Clock and timestamp contract tests: Kevlar vs Linux.
// Compile: musl-gcc -static -o test_clock test_clock.c
// Run as init or from shell on both Linux and Kevlar.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>

static int pass_count = 0, fail_count = 0;

#define TEST(name) do { printf("TEST %s: ", name); } while(0)
#define PASS() do { printf("PASS\n"); pass_count++; } while(0)
#define FAIL(fmt, ...) do { printf("FAIL " fmt "\n", ##__VA_ARGS__); fail_count++; } while(0)

// ── Test 1: CLOCK_REALTIME returns plausible time ──────────────
static void test_realtime_plausible(void) {
    TEST("clock_gettime CLOCK_REALTIME plausible");
    struct timespec ts;
    if (clock_gettime(CLOCK_REALTIME, &ts) != 0) {
        FAIL("clock_gettime failed: %s", strerror(errno));
        return;
    }
    // Should be after 2024-01-01 (1704067200) and before 2030-01-01 (1893456000)
    if (ts.tv_sec > 1704067200 && ts.tv_sec < 1893456000) {
        PASS();
    } else {
        FAIL("tv_sec=%ld (expected 2024-2030 range)", (long)ts.tv_sec);
    }
}

// ── Test 2: CLOCK_REALTIME nanoseconds non-zero ────────────────
static void test_realtime_nsec(void) {
    TEST("clock_gettime CLOCK_REALTIME nsec non-zero");
    // Sample multiple times — at least one should have non-zero nsec
    int found_nonzero = 0;
    for (int i = 0; i < 100; i++) {
        struct timespec ts;
        clock_gettime(CLOCK_REALTIME, &ts);
        if (ts.tv_nsec != 0) {
            found_nonzero = 1;
            break;
        }
    }
    if (found_nonzero) {
        PASS();
    } else {
        FAIL("tv_nsec was 0 in all 100 samples");
    }
}

// ── Test 3: CLOCK_MONOTONIC increases ──────────────────────────
static void test_monotonic_increases(void) {
    TEST("clock_gettime CLOCK_MONOTONIC monotonic");
    struct timespec a, b;
    clock_gettime(CLOCK_MONOTONIC, &a);
    // Busy-wait a tiny bit
    for (volatile int i = 0; i < 10000; i++);
    clock_gettime(CLOCK_MONOTONIC, &b);
    long diff_ns = (b.tv_sec - a.tv_sec) * 1000000000L + (b.tv_nsec - a.tv_nsec);
    if (diff_ns > 0) {
        PASS();
    } else {
        FAIL("not monotonic: a=%ld.%09ld b=%ld.%09ld diff=%ldns",
             (long)a.tv_sec, a.tv_nsec, (long)b.tv_sec, b.tv_nsec, diff_ns);
    }
}

// ── Test 4: CLOCK_REALTIME vs gettimeofday agreement ───────────
static void test_realtime_vs_gettimeofday(void) {
    TEST("clock_gettime vs gettimeofday agreement");
    struct timespec ts;
    struct timeval tv;
    clock_gettime(CLOCK_REALTIME, &ts);
    gettimeofday(&tv, NULL);
    long diff_sec = (long)ts.tv_sec - (long)tv.tv_sec;
    if (diff_sec >= -1 && diff_sec <= 1) {
        PASS();
    } else {
        FAIL("diff=%ld sec (ts=%ld tv=%ld)", diff_sec, (long)ts.tv_sec, (long)tv.tv_sec);
    }
}

// ── Test 5: gettimeofday microseconds non-zero ────────────────
static void test_gettimeofday_usec(void) {
    TEST("gettimeofday usec non-zero");
    int found_nonzero = 0;
    for (int i = 0; i < 100; i++) {
        struct timeval tv;
        gettimeofday(&tv, NULL);
        if (tv.tv_usec != 0) {
            found_nonzero = 1;
            break;
        }
    }
    if (found_nonzero) {
        PASS();
    } else {
        FAIL("tv_usec was 0 in all 100 samples");
    }
}

// ── Test 6: New file mtime matches wall clock ──────────────────
static void test_file_mtime_matches_clock(void) {
    TEST("new file mtime matches wall clock");
    struct timespec before, after;
    clock_gettime(CLOCK_REALTIME, &before);

    const char *path = "/tmp/test_clock_file";
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) {
        FAIL("open failed: %s", strerror(errno));
        return;
    }
    write(fd, "hello", 5);
    close(fd);

    clock_gettime(CLOCK_REALTIME, &after);

    struct stat st;
    if (stat(path, &st) != 0) {
        FAIL("stat failed: %s", strerror(errno));
        unlink(path);
        return;
    }

    // mtime should be between before and after (within 2 seconds tolerance)
    if (st.st_mtime >= before.tv_sec - 1 && st.st_mtime <= after.tv_sec + 1) {
        PASS();
    } else {
        FAIL("mtime=%ld before=%ld after=%ld",
             (long)st.st_mtime, (long)before.tv_sec, (long)after.tv_sec);
    }
    unlink(path);
}

// ── Test 7: stat() reports nanosecond timestamps ───────────────
static void test_stat_nsec(void) {
    TEST("stat mtime_nsec non-zero on new file");
    const char *path = "/tmp/test_clock_nsec";
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) {
        FAIL("open failed: %s", strerror(errno));
        return;
    }
    write(fd, "x", 1);
    close(fd);

    struct stat st;
    stat(path, &st);

    // On Linux with ext4/tmpfs, st_mtim.tv_nsec is usually non-zero.
    // On Kevlar, if nsec is always 0, that's a known gap.
    if (st.st_mtim.tv_nsec != 0) {
        PASS();
    } else {
        FAIL("st_mtim.tv_nsec = 0 (no nanosecond precision)");
    }
    unlink(path);
}

// ── Test 8: utimensat UTIME_NOW sets current time ──────────────
static void test_utimensat_utime_now(void) {
    TEST("utimensat UTIME_NOW");
    const char *path = "/tmp/test_clock_utime";
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) {
        FAIL("open failed: %s", strerror(errno));
        return;
    }
    close(fd);

    // Set mtime to epoch
    struct timespec times[2] = {
        { .tv_sec = 1000, .tv_nsec = 0 },        // atime
        { .tv_sec = 1000, .tv_nsec = 0 },         // mtime
    };
    utimensat(AT_FDCWD, path, times, 0);

    struct stat st1;
    stat(path, &st1);
    if (st1.st_mtime != 1000) {
        FAIL("utimensat set to 1000 failed (got %ld)", (long)st1.st_mtime);
        unlink(path);
        return;
    }

    // Now set to UTIME_NOW
    struct timespec now_times[2] = {
        { .tv_sec = 0, .tv_nsec = ((1L << 30) - 1) },  // UTIME_NOW
        { .tv_sec = 0, .tv_nsec = ((1L << 30) - 1) },  // UTIME_NOW
    };
    struct timespec before;
    clock_gettime(CLOCK_REALTIME, &before);
    utimensat(AT_FDCWD, path, now_times, 0);

    struct stat st2;
    stat(path, &st2);
    if (st2.st_mtime >= before.tv_sec - 1 && st2.st_mtime <= before.tv_sec + 2) {
        PASS();
    } else {
        FAIL("UTIME_NOW mtime=%ld expected ~%ld", (long)st2.st_mtime, (long)before.tv_sec);
    }
    unlink(path);
}

// ── Test 9: write() updates mtime ─────────────────────────────
static void test_write_updates_mtime(void) {
    TEST("write() updates mtime");
    const char *path = "/tmp/test_clock_write";
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) {
        FAIL("open failed: %s", strerror(errno));
        return;
    }
    write(fd, "a", 1);
    close(fd);

    // Set mtime to epoch
    struct timespec times[2] = {
        { .tv_sec = 1000, .tv_nsec = 0 },
        { .tv_sec = 1000, .tv_nsec = 0 },
    };
    utimensat(AT_FDCWD, path, times, 0);

    struct stat st1;
    stat(path, &st1);

    // Sleep briefly then write again
    usleep(100000); // 100ms

    fd = open(path, O_WRONLY | O_APPEND);
    if (fd < 0) {
        FAIL("reopen failed: %s", strerror(errno));
        unlink(path);
        return;
    }
    write(fd, "b", 1);
    close(fd);

    struct stat st2;
    stat(path, &st2);
    if (st2.st_mtime > st1.st_mtime) {
        PASS();
    } else {
        FAIL("mtime not updated: before=%ld after=%ld",
             (long)st1.st_mtime, (long)st2.st_mtime);
    }
    unlink(path);
}

// ── Test 10: clock_getres returns valid resolution ─────────────
static void test_clock_getres(void) {
    TEST("clock_getres CLOCK_REALTIME");
    struct timespec res;
    if (clock_getres(CLOCK_REALTIME, &res) != 0) {
        FAIL("clock_getres failed: %s", strerror(errno));
        return;
    }
    // Resolution should be at most 10ms (100 Hz tick) = 10,000,000 ns
    if (res.tv_sec == 0 && res.tv_nsec > 0 && res.tv_nsec <= 10000000) {
        PASS();
    } else {
        FAIL("res=%ld.%09ld", (long)res.tv_sec, res.tv_nsec);
    }
}

// ── Test 11: CLOCK_MONOTONIC_RAW available ─────────────────────
static void test_clock_monotonic_raw(void) {
    TEST("clock_gettime CLOCK_MONOTONIC_RAW");
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC_RAW, &ts) != 0) {
        FAIL("clock_gettime failed: %s", strerror(errno));
        return;
    }
    if (ts.tv_sec >= 0) {
        PASS();
    } else {
        FAIL("tv_sec=%ld", (long)ts.tv_sec);
    }
}

// ── Test 12: CLOCK_BOOTTIME available ──────────────────────────
static void test_clock_boottime(void) {
    TEST("clock_gettime CLOCK_BOOTTIME");
    struct timespec ts;
    if (clock_gettime(CLOCK_BOOTTIME, &ts) != 0) {
        FAIL("clock_gettime failed: %s", strerror(errno));
        return;
    }
    if (ts.tv_sec >= 0) {
        PASS();
    } else {
        FAIL("tv_sec=%ld", (long)ts.tv_sec);
    }
}

// ── Test 13: mkdir updates parent mtime ────────────────────────
static void test_mkdir_updates_parent_mtime(void) {
    TEST("mkdir updates parent directory mtime");
    const char *dir = "/tmp/test_clock_dir";
    const char *sub = "/tmp/test_clock_dir/sub";
    mkdir(dir, 0755);

    // Set dir mtime to epoch
    struct timespec times[2] = {
        { .tv_sec = 1000, .tv_nsec = 0 },
        { .tv_sec = 1000, .tv_nsec = 0 },
    };
    utimensat(AT_FDCWD, dir, times, 0);

    struct stat st1;
    stat(dir, &st1);

    usleep(100000);
    mkdir(sub, 0755);

    struct stat st2;
    stat(dir, &st2);

    if (st2.st_mtime > st1.st_mtime) {
        PASS();
    } else {
        FAIL("parent mtime not updated: before=%ld after=%ld",
             (long)st1.st_mtime, (long)st2.st_mtime);
    }
    rmdir(sub);
    rmdir(dir);
}

int main(void) {
    printf("=== Clock Contract Tests ===\n");

    test_realtime_plausible();
    test_realtime_nsec();
    test_monotonic_increases();
    test_realtime_vs_gettimeofday();
    test_gettimeofday_usec();
    test_file_mtime_matches_clock();
    test_stat_nsec();
    test_utimensat_utime_now();
    test_write_updates_mtime();
    test_clock_getres();
    test_clock_monotonic_raw();
    test_clock_boottime();
    test_mkdir_updates_parent_mtime();

    printf("\n=== Results: %d PASS, %d FAIL ===\n", pass_count, fail_count);
    if (fail_count > 0) {
        printf("TEST_FAIL\n");
    } else {
        printf("TEST_PASS\n");
    }
    return fail_count > 0 ? 1 : 0;
}
