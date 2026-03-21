#include <fcntl.h>
#include <unistd.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <string.h>
#include <stdio.h>
#include <errno.h>

int main(void) {
    write(1, "=== apk write test ===\n", 23);
    mkdir("/mnt", 0755);
    mount("tmpfs", "/mnt", "tmpfs", 0, NULL);
    mkdir("/mnt/r", 0755);
    mount("none", "/mnt/r", "ext4", 0, NULL);
    mkdir("/mnt/r/old", 0755);
    syscall(155, "/mnt/r", "/mnt/r/old");
    chdir("/");

    char buf[256];
    int n;

    // Test creating files in key directories
    const char *dirs[] = {
        "/var/cache/apk",
        "/lib/apk/db",
        "/tmp",
        NULL
    };
    for (int i = 0; dirs[i]; i++) {
        char path[256];
        snprintf(path, sizeof(path), "%s/test_write", dirs[i]);
        errno = 0;
        int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
        n = snprintf(buf, sizeof(buf), "create %s: fd=%d errno=%d (%s)\n",
            path, fd, errno, strerror(errno));
        write(1, buf, n);
        if (fd >= 0) {
            write(fd, "test\n", 5);
            close(fd);
            unlink(path);
        }
    }

    // Check directory permissions
    for (int i = 0; dirs[i]; i++) {
        struct stat st;
        stat(dirs[i], &st);
        n = snprintf(buf, sizeof(buf), "stat %s: mode=%o uid=%d gid=%d\n",
            dirs[i], st.st_mode, st.st_uid, st.st_gid);
        write(1, buf, n);
    }

    write(1, "=== done ===\n", 13);
    return 0;
}
