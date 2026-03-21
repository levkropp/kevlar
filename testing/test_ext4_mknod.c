// Test ext4 extent write support and mknod device dispatch.
// Tests: mount ext4, create/write/read/truncate/unlink extent-based files,
// mkdir/rmdir, symlinks, mknod device nodes.
#include <fcntl.h>
#include <unistd.h>
#include <sys/stat.h>
#include <sys/mount.h>
#include <string.h>
#include <dirent.h>
#include <stdio.h>

static int pass_count = 0;
static int fail_count = 0;

static void check(const char *msg, int ok) {
    if (ok) { write(1, "PASS ", 5); pass_count++; }
    else    { write(1, "FAIL ", 5); fail_count++; }
    write(1, msg, strlen(msg));
    write(1, "\n", 1);
}

int main(void) {
    write(1, "TEST_START ext4_mknod\n", 22);

    // ── ext4 mount and basic I/O ──────────────────────────────────

    // Mount ext4 (kernel reports as "ext4" label, uses "ext2" mount type)
    mkdir("/mnt", 0755);
    int r = mount("/dev/vda", "/mnt", "ext2", 0, NULL);
    check("mount_ext4", r == 0);
    if (r != 0) {
        write(1, "TEST_END\n", 9);
        return 1;
    }

    // Create a new file (should use extent tree on ext4)
    int fd = open("/mnt/extent_test.txt", O_WRONLY | O_CREAT | O_TRUNC, 0644);
    check("create_extent_file", fd >= 0);
    if (fd >= 0) {
        const char *msg = "ext4 extent write works!\n";
        int n = write(fd, msg, strlen(msg));
        check("write_extent_file", n == (int)strlen(msg));
        close(fd);
    }

    // Read it back
    fd = open("/mnt/extent_test.txt", O_RDONLY);
    check("open_extent_read", fd >= 0);
    if (fd >= 0) {
        char buf[128] = {0};
        int n = read(fd, buf, sizeof(buf));
        check("read_extent_file", n > 0 && strncmp(buf, "ext4 extent write", 17) == 0);
        close(fd);
    }

    // Write a larger file (multiple blocks to test extent extension)
    fd = open("/mnt/multiblock.dat", O_WRONLY | O_CREAT | O_TRUNC, 0644);
    check("create_multiblock", fd >= 0);
    if (fd >= 0) {
        char block[4096];
        memset(block, 'A', sizeof(block));
        int total = 0;
        // Write 16 blocks = 64KB (should be one contiguous extent)
        for (int i = 0; i < 16; i++) {
            block[0] = '0' + (i % 10); // tag each block
            int n = write(fd, block, sizeof(block));
            if (n > 0) total += n;
        }
        check("write_multiblock", total == 16 * 4096);
        close(fd);
    }

    // Read back and verify
    fd = open("/mnt/multiblock.dat", O_RDONLY);
    check("open_multiblock_read", fd >= 0);
    if (fd >= 0) {
        char buf[4096];
        // Read first block
        int n = read(fd, buf, sizeof(buf));
        check("read_first_block", n == 4096 && buf[0] == '0' && buf[1] == 'A');
        // Seek to last block
        lseek(fd, 15 * 4096, 0);
        n = read(fd, buf, sizeof(buf));
        check("read_last_block", n == 4096 && buf[0] == '5' && buf[1] == 'A');
        close(fd);
    }

    // Stat the file — verify size
    struct stat st;
    r = stat("/mnt/multiblock.dat", &st);
    check("stat_multiblock", r == 0 && st.st_size == 16 * 4096);

    // ── Truncate ──────────────────────────────────────────────────

    // Truncate to 0 (tests extent-aware truncate)
    fd = open("/mnt/extent_test.txt", O_WRONLY | O_TRUNC);
    check("truncate_to_zero", fd >= 0);
    if (fd >= 0) {
        const char *msg2 = "rewritten after truncate\n";
        write(fd, msg2, strlen(msg2));
        close(fd);
    }
    fd = open("/mnt/extent_test.txt", O_RDONLY);
    if (fd >= 0) {
        char buf[128] = {0};
        int n = read(fd, buf, sizeof(buf));
        check("read_after_truncate", n > 0 && strncmp(buf, "rewritten", 9) == 0);
        close(fd);
    }

    // ── Directory operations ──────────────────────────────────────

    // Clean up from previous runs
    unlink("/mnt/testdir/inner.txt");
    rmdir("/mnt/testdir");

    r = mkdir("/mnt/testdir", 0755);
    check("mkdir_ext4", r == 0);

    fd = open("/mnt/testdir/inner.txt", O_WRONLY | O_CREAT, 0644);
    check("create_in_dir", fd >= 0);
    if (fd >= 0) {
        write(fd, "nested\n", 7);
        close(fd);
    }

    DIR *d = opendir("/mnt/testdir");
    check("opendir", d != NULL);
    if (d) {
        int count = 0;
        struct dirent *ent;
        while ((ent = readdir(d)) != NULL) {
            if (strcmp(ent->d_name, ".") != 0 && strcmp(ent->d_name, "..") != 0)
                count++;
        }
        closedir(d);
        check("readdir_count", count == 1);
    }

    // ── Symlink ───────────────────────────────────────────────────

    unlink("/mnt/link.txt");
    r = symlink("extent_test.txt", "/mnt/link.txt");
    check("symlink", r == 0);
    char linkbuf[128] = {0};
    int n = readlink("/mnt/link.txt", linkbuf, sizeof(linkbuf));
    check("readlink", n > 0 && strcmp(linkbuf, "extent_test.txt") == 0);

    // ── Unlink + rmdir ────────────────────────────────────────────

    r = unlink("/mnt/multiblock.dat");
    check("unlink_multiblock", r == 0);
    fd = open("/mnt/multiblock.dat", O_RDONLY);
    check("unlinked_gone", fd < 0);

    unlink("/mnt/testdir/inner.txt");
    r = rmdir("/mnt/testdir");
    check("rmdir_ext4", r == 0);

    // ── mknod device dispatch ─────────────────────────────────────

    // mknod /dev/null equivalent on tmpfs
    // Note: /tmp is tmpfs, so mknod works there
    r = mknod("/tmp/testnull", S_IFCHR | 0666, (1 << 8) | 3); // major=1, minor=3 = /dev/null
    check("mknod_null", r == 0);

    // Write to it (should succeed, data discarded)
    fd = open("/tmp/testnull", O_WRONLY);
    check("open_mknod_null", fd >= 0);
    if (fd >= 0) {
        n = write(fd, "discard me\n", 11);
        check("write_mknod_null", n == 11);
        close(fd);
    }

    // Read from it (should return 0 = EOF)
    fd = open("/tmp/testnull", O_RDONLY);
    check("open_mknod_null_read", fd >= 0);
    if (fd >= 0) {
        char buf[16];
        n = read(fd, buf, sizeof(buf));
        check("read_mknod_null_eof", n == 0);
        close(fd);
    }

    // mknod /dev/zero equivalent
    r = mknod("/tmp/testzero", S_IFCHR | 0666, (1 << 8) | 5); // major=1, minor=5 = /dev/zero
    check("mknod_zero", r == 0);

    fd = open("/tmp/testzero", O_RDONLY);
    check("open_mknod_zero", fd >= 0);
    if (fd >= 0) {
        char buf[16] = {0xFF, 0xFF, 0xFF, 0xFF};
        n = read(fd, buf, 4);
        check("read_mknod_zero", n == 4 && buf[0] == 0 && buf[1] == 0 && buf[2] == 0 && buf[3] == 0);
        close(fd);
    }

    // ── Cleanup ───────────────────────────────────────────────────

    unlink("/mnt/extent_test.txt");
    unlink("/mnt/link.txt");

    // ── Summary ───────────────────────────────────────────────────

    char summary[128];
    int slen = snprintf(summary, sizeof(summary),
        "ext4_mknod: %d passed, %d failed\n", pass_count, fail_count);
    write(1, summary, slen);

    write(1, "TEST_END\n", 9);
    return fail_count > 0 ? 1 : 0;
}
