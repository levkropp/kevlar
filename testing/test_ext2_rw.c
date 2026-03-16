// Minimal test for ext2 read-write operations via the VFS.
// Tests what apk.static would do: open dirs, read files, write files.
#include <fcntl.h>
#include <unistd.h>
#include <sys/stat.h>
#include <sys/mount.h>
#include <string.h>
#include <dirent.h>
#include <stdio.h>

static void check(const char *msg, int ok) {
    if (ok) write(1, "PASS ", 5);
    else    write(1, "FAIL ", 5);
    write(1, msg, strlen(msg));
    write(1, "\n", 1);
}

int main(void) {
    write(1, "TEST_START ext2_rw\n", 19);

    // Mount ext2
    int r = mount("/dev/vda", "/mnt", "ext2", 0, NULL);
    check("mount_ext2", r == 0);

    // Create a file
    int fd = open("/mnt/hello.txt", O_WRONLY | O_CREAT | O_TRUNC, 0644);
    check("create_file", fd >= 0);
    if (fd >= 0) {
        const char *msg = "hello from kevlar ext2 rw\n";
        int n = write(fd, msg, strlen(msg));
        check("write_file", n == (int)strlen(msg));
        close(fd);
    }

    // Read it back
    fd = open("/mnt/hello.txt", O_RDONLY);
    check("open_for_read", fd >= 0);
    if (fd >= 0) {
        char buf[128] = {0};
        int n = read(fd, buf, sizeof(buf));
        check("read_file", n > 0 && strncmp(buf, "hello from kevlar", 17) == 0);
        close(fd);
    }

    // Create a directory (clean up from previous runs first)
    unlink("/mnt/testdir/inner.txt");
    rmdir("/mnt/testdir");
    r = mkdir("/mnt/testdir", 0755);
    check("mkdir", r == 0);

    // Create a file in the directory
    fd = open("/mnt/testdir/inner.txt", O_WRONLY | O_CREAT, 0644);
    check("create_in_dir", fd >= 0);
    if (fd >= 0) {
        write(fd, "inner\n", 6);
        close(fd);
    }

    // List directory
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

    // Create and read symlink
    unlink("/mnt/newlink.txt"); // clean up from previous runs
    r = symlink("hello.txt", "/mnt/newlink.txt");
    check("symlink", r == 0);
    char linkbuf[128] = {0};
    int n = readlink("/mnt/newlink.txt", linkbuf, sizeof(linkbuf));
    check("readlink", n > 0 && strcmp(linkbuf, "hello.txt") == 0);

    // Read through symlink
    fd = open("/mnt/newlink.txt", O_RDONLY);
    check("open_symlink", fd >= 0);
    if (fd >= 0) {
        char buf[128] = {0};
        read(fd, buf, sizeof(buf));
        check("read_via_symlink", strncmp(buf, "hello from kevlar", 17) == 0);
        close(fd);
    }

    // Unlink
    r = unlink("/mnt/hello.txt");
    check("unlink", r == 0);
    fd = open("/mnt/hello.txt", O_RDONLY);
    check("unlinked_gone", fd < 0);

    // Truncate
    unlink("/mnt/trunc.txt");
    unlink("/mnt/renamed.txt");
    fd = open("/mnt/trunc.txt", O_WRONLY | O_CREAT, 0644);
    if (fd >= 0) {
        write(fd, "1234567890", 10);
        ftruncate(fd, 5);
        close(fd);
    }
    fd = open("/mnt/trunc.txt", O_RDONLY);
    if (fd >= 0) {
        char buf[32] = {0};
        int n = read(fd, buf, sizeof(buf));
        check("truncate", n == 5 && memcmp(buf, "12345", 5) == 0);
        close(fd);
    }

    // Rename
    r = rename("/mnt/trunc.txt", "/mnt/renamed.txt");
    check("rename", r == 0);
    fd = open("/mnt/renamed.txt", O_RDONLY);
    check("renamed_exists", fd >= 0);
    if (fd >= 0) close(fd);

    // rmdir (need to unlink contents first)
    unlink("/mnt/testdir/inner.txt");
    r = rmdir("/mnt/testdir");
    check("rmdir", r == 0);

    write(1, "TEST_END\n", 9);
    return 0;
}
