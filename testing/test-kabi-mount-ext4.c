// Phase 13: userspace integration test for the kABI-mounted ext4
// filesystem.  Runs as PID 1 (init=/bin/test-kabi-mount-ext4).
// Exercises the full syscall path: mount(2) → opendir/readdir →
// open/read against /mnt/ext4/{hello.txt,info.txt}.
//
// ext4.ko is loaded unconditionally at boot now (Phase 13), so no
// special cmdline flags are required.  The test fixture is
// /dev/vda (= build/test-fixtures/test.ext4 attached as a virtio
// disk).
#include <fcntl.h>
#include <unistd.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <dirent.h>
#include <string.h>
#include <stdio.h>
#include <errno.h>

static int pass_count = 0;
static int fail_count = 0;

static void check(const char *msg, int ok) {
    if (ok) { write(1, "PASS ", 5); pass_count++; }
    else    { write(1, "FAIL ", 5); fail_count++; }
    write(1, msg, strlen(msg));
    write(1, "\n", 1);
}

int main(void) {
    write(1, "TEST_START kabi_mount_ext4\n", 27);

    /* 1. Mount ext4 at /mnt/ext4.  /mnt and /mnt/ext4 are
     *    pre-created by tools/build-initramfs.py (initramfs is RO at
     *    runtime, so we can't mkdir at boot). */
    int r = mount("/dev/vda", "/mnt/ext4", "ext4",
                  MS_RDONLY, NULL);
    int em = errno;
    check("mount_ext4", r == 0);
    if (r != 0) {
        char line[64];
        int len = snprintf(line, sizeof(line),
                           "mount r=%d errno=%d\n", r, em);
        write(1, line, len);
        write(1, "TEST_FAIL kabi_mount_ext4\n", 26);
        write(1, "TEST_END\n", 9);
        return 1;
    }

    /* 2. opendir + readdir, collect names. */
    DIR *d = opendir("/mnt/ext4");
    check("opendir_mnt_ext4", d != NULL);
    int saw_hello = 0, saw_info = 0;
    if (d) {
        struct dirent *ent;
        while ((ent = readdir(d)) != NULL) {
            if (!strcmp(ent->d_name, "hello.txt")) saw_hello = 1;
            if (!strcmp(ent->d_name, "info.txt"))  saw_info  = 1;
        }
        closedir(d);
    }
    check("readdir_hello.txt", saw_hello);
    check("readdir_info.txt",  saw_info);

    /* 3. open + read hello.txt, exact match. */
    int fd = open("/mnt/ext4/hello.txt", O_RDONLY);
    check("open_hello.txt", fd >= 0);
    if (fd >= 0) {
        char buf[64] = {0};
        int n = read(fd, buf, sizeof(buf) - 1);
        const char *expect = "hello from kABI-mounted ext4!\n";
        check("read_hello.txt_size", n == (int)strlen(expect));
        check("read_hello.txt_bytes",
              n > 0 && !strncmp(buf, expect, n));
        close(fd);
    }

    /* 4. read at non-zero offset. */
    fd = open("/mnt/ext4/hello.txt", O_RDONLY);
    if (fd >= 0) {
        lseek(fd, 6, SEEK_SET);
        char buf[16] = {0};
        int n = read(fd, buf, sizeof(buf) - 1);
        check("read_hello.txt_offset_6",
              n == 15 && !strncmp(buf, "from kABI-mount", 15));
        close(fd);
    }

    /* 5. summary. */
    char line[64];
    int len = snprintf(line, sizeof(line),
                       "RESULTS: %d passed, %d failed\n",
                       pass_count, fail_count);
    write(1, line, len);
    if (fail_count == 0) write(1, "TEST_PASS kabi_mount_ext4\n", 26);
    else                 write(1, "TEST_FAIL kabi_mount_ext4\n", 26);
    write(1, "TEST_END\n", 9);
    return fail_count == 0 ? 0 : 1;
}
