#include <unistd.h>
#include <sys/mount.h>
#include <sys/wait.h>
#include <sys/stat.h>
#include <string.h>
#include <stdio.h>
#include <fcntl.h>
#include <errno.h>
#include <dirent.h>

static void msg(const char *s) { write(1, s, strlen(s)); }

int main(void) {
    msg("=== libz test ===\n");

    mkdir("/mnt", 0755);
    mount("tmpfs", "/mnt", "tmpfs", 0, NULL);
    mkdir("/mnt/root", 0755);
    mount("none", "/mnt/root", "ext4", 0, NULL);
    mkdir("/mnt/root/proc", 0755);
    mount("proc", "/mnt/root/proc", "proc", 0, NULL);
    mkdir("/mnt/root/dev", 0755);
    mount("devtmpfs", "/mnt/root/dev", "devtmpfs", 0, NULL);

    mkdir("/mnt/root/oldroot", 0755);
    syscall(155, "/mnt/root", "/mnt/root/oldroot");
    chdir("/");

    // Check if /usr/lib/libz.so.1 exists
    char buf[256];
    int fd = open("/usr/lib/libz.so.1", O_RDONLY);
    int n = snprintf(buf, sizeof(buf), "open /usr/lib/libz.so.1: fd=%d errno=%d\n", fd, errno);
    write(1, buf, n);
    if (fd >= 0) {
        // Read first 4 bytes to verify it's an ELF
        char hdr[4];
        read(fd, hdr, 4);
        n = snprintf(buf, sizeof(buf), "  header: %02x %02x %02x %02x (%s)\n",
            (unsigned char)hdr[0], (unsigned char)hdr[1],
            (unsigned char)hdr[2], (unsigned char)hdr[3],
            hdr[0] == 0x7f ? "ELF" : "not ELF");
        write(1, buf, n);
        close(fd);
    }

    // Check if it's a symlink
    char target[256] = {0};
    int lr = readlink("/usr/lib/libz.so.1", target, sizeof(target) - 1);
    n = snprintf(buf, sizeof(buf), "readlink /usr/lib/libz.so.1: %d → '%s'\n",
        lr, lr > 0 ? target : "(failed)");
    write(1, buf, n);

    // List /usr/lib/ for libz*
    msg("ls /usr/lib/libz*:\n");
    DIR *d = opendir("/usr/lib");
    if (d) {
        struct dirent *ent;
        while ((ent = readdir(d)) != NULL) {
            if (strncmp(ent->d_name, "libz", 4) == 0) {
                n = snprintf(buf, sizeof(buf), "  %s\n", ent->d_name);
                write(1, buf, n);
            }
        }
        closedir(d);
    } else {
        n = snprintf(buf, sizeof(buf), "  opendir failed: %d\n", errno);
        write(1, buf, n);
    }

    // Also check /lib/
    msg("ls /lib/libz*:\n");
    d = opendir("/lib");
    if (d) {
        struct dirent *ent;
        while ((ent = readdir(d)) != NULL) {
            if (strncmp(ent->d_name, "libz", 4) == 0) {
                n = snprintf(buf, sizeof(buf), "  %s\n", ent->d_name);
                write(1, buf, n);
            }
        }
        closedir(d);
    } else {
        n = snprintf(buf, sizeof(buf), "  opendir failed: %d\n", errno);
        write(1, buf, n);
    }

    // Try running apk
    msg("exec apk --version:\n");
    int pid = fork();
    if (pid == 0) {
        char *argv[] = { "apk", "--version", NULL };
        char *envp[] = { "PATH=/usr/sbin:/usr/bin:/sbin:/bin",
                         "LD_LIBRARY_PATH=/usr/lib:/lib", NULL };
        execve("/sbin/apk", argv, envp);
        n = snprintf(buf, sizeof(buf), "  execve failed: %d\n", errno);
        write(1, buf, n);
        _exit(1);
    }
    int status;
    waitpid(pid, &status, 0);
    n = snprintf(buf, sizeof(buf), "  status=0x%x\n", status);
    write(1, buf, n);

    msg("=== done ===\n");
    return 0;
}
