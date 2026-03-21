#include <fcntl.h>
#include <unistd.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <string.h>
#include <stdio.h>
#include <errno.h>

int main(void) {
    write(1, "=== apk interactive test ===\n", 29);

    mkdir("/mnt", 0755);
    mount("tmpfs", "/mnt", "tmpfs", 0, NULL);
    mkdir("/mnt/root", 0755);
    mount("none", "/mnt/root", "ext4", 0, NULL);
    mkdir("/mnt/root/proc", 0755);
    mount("proc", "/mnt/root/proc", "proc", 0, NULL);
    mkdir("/mnt/root/dev", 0755);
    mount("devtmpfs", "/mnt/root/dev", "devtmpfs", 0, NULL);
    mkdir("/mnt/root/run", 0755);
    mount("tmpfs", "/mnt/root/run", "tmpfs", 0, NULL);
    mkdir("/mnt/root/tmp", 01777);
    mount("tmpfs", "/mnt/root/tmp", "tmpfs", 0, NULL);

    mkdir("/mnt/root/oldroot", 0755);
    syscall(155, "/mnt/root", "/mnt/root/oldroot");
    chdir("/");

    // Run OpenRC sysinit (like the real boot)
    write(1, "running openrc sysinit...\n", 25);
    int pid = fork();
    if (pid == 0) {
        char *argv[] = { "openrc", "sysinit", NULL };
        char *envp[] = { "PATH=/usr/sbin:/usr/bin:/sbin:/bin", NULL };
        execve("/sbin/openrc", argv, envp);
        _exit(1);
    }
    int status;
    waitpid(pid, &status, 0);

    // Now try apk (same as login shell would)
    char buf[256];
    int n;

    write(1, "checking /lib/apk/db/lock...\n", 28);
    struct stat st;
    int r = stat("/lib/apk/db/lock", &st);
    n = snprintf(buf, sizeof(buf), "  stat: %d errno=%d mode=%o\n",
        r, errno, r == 0 ? st.st_mode : 0);
    write(1, buf, n);

    // Try to open it for writing
    int fd = open("/lib/apk/db/lock", O_RDWR);
    n = snprintf(buf, sizeof(buf), "  open(RDWR): fd=%d errno=%d (%s)\n",
        fd, errno, strerror(errno));
    write(1, buf, n);
    if (fd >= 0) close(fd);

    // Check if / is writable
    write(1, "checking write to /:\n", 21);
    fd = open("/test_write", O_WRONLY | O_CREAT, 0644);
    n = snprintf(buf, sizeof(buf), "  create /test_write: fd=%d errno=%d (%s)\n",
        fd, errno, strerror(errno));
    write(1, buf, n);
    if (fd >= 0) { close(fd); unlink("/test_write"); }

    write(1, "running apk update...\n", 22);
    pid = fork();
    if (pid == 0) {
        char *argv[] = { "apk", "update", NULL };
        char *envp[] = { "PATH=/usr/sbin:/usr/bin:/sbin:/bin", NULL };
        execve("/sbin/apk", argv, envp);
        _exit(1);
    }
    waitpid(pid, &status, 0);
    n = snprintf(buf, sizeof(buf), "apk status: 0x%x\n", status);
    write(1, buf, n);

    write(1, "=== done ===\n", 13);
    return 0;
}
