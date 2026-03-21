#include <fcntl.h>
#include <unistd.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <string.h>
#include <stdio.h>
#include <errno.h>

int main(void) {
    write(1, "=== apk update test ===\n", 24);

    mkdir("/mnt", 0755);
    mount("tmpfs", "/mnt", "tmpfs", 0, NULL);
    mkdir("/mnt/root", 0755);
    mount("none", "/mnt/root", "ext4", 0, NULL);

    // Mount essential filesystems inside new root
    mkdir("/mnt/root/proc", 0755);
    mount("proc", "/mnt/root/proc", "proc", 0, NULL);
    mkdir("/mnt/root/sys", 0755);
    mount("sysfs", "/mnt/root/sys", "sysfs", 0, NULL);
    mkdir("/mnt/root/dev", 0755);
    mount("devtmpfs", "/mnt/root/dev", "devtmpfs", 0, NULL);
    mkdir("/mnt/root/run", 0755);
    mount("tmpfs", "/mnt/root/run", "tmpfs", 0, NULL);
    mkdir("/mnt/root/tmp", 01777);
    mount("tmpfs", "/mnt/root/tmp", "tmpfs", 0, NULL);

    // pivot_root
    mkdir("/mnt/root/oldroot", 0755);
    syscall(155, "/mnt/root", "/mnt/root/oldroot");
    chdir("/");

    // Simulate what login does: setsid, setgroups, etc.
    // Then run apk as a child (same as typing it at the shell)
    char buf[256];
    int n;

    // Test 1: apk --version (should work)
    write(1, "test1: apk --version\n", 21);
    int pid = fork();
    if (pid == 0) {
        char *argv[] = { "apk", "--version", NULL };
        char *envp[] = { "PATH=/usr/sbin:/usr/bin:/sbin:/bin", "HOME=/root", NULL };
        execve("/sbin/apk", argv, envp);
        _exit(1);
    }
    int status;
    waitpid(pid, &status, 0);
    n = snprintf(buf, sizeof(buf), "  status=0x%x\n", status);
    write(1, buf, n);

    // Test 2: apk info (reads database)
    write(1, "test2: apk info\n", 16);
    pid = fork();
    if (pid == 0) {
        char *argv[] = { "apk", "info", NULL };
        char *envp[] = { "PATH=/usr/sbin:/usr/bin:/sbin:/bin", "HOME=/root", NULL };
        execve("/sbin/apk", argv, envp);
        _exit(1);
    }
    waitpid(pid, &status, 0);
    n = snprintf(buf, sizeof(buf), "  status=0x%x\n", status);
    write(1, buf, n);

    // Test 3: apk update (writes to database - the failing case)
    write(1, "test3: apk update\n", 18);
    pid = fork();
    if (pid == 0) {
        char *argv[] = { "apk", "update", NULL };
        char *envp[] = { "PATH=/usr/sbin:/usr/bin:/sbin:/bin", "HOME=/root", NULL };
        execve("/sbin/apk", argv, envp);
        _exit(1);
    }
    waitpid(pid, &status, 0);
    n = snprintf(buf, sizeof(buf), "  status=0x%x\n", status);
    write(1, buf, n);

    // Test 4: check if the lock file exists and is writable
    write(1, "test4: lock file check\n", 22);
    int fd = open("/lib/apk/db/lock", O_WRONLY);
    n = snprintf(buf, sizeof(buf), "  open(O_WRONLY) fd=%d errno=%d (%s)\n",
        fd, errno, strerror(errno));
    write(1, buf, n);
    if (fd >= 0) close(fd);

    fd = open("/lib/apk/db/lock", O_RDWR | O_CREAT, 0600);
    n = snprintf(buf, sizeof(buf), "  open(O_RDWR|O_CREAT) fd=%d errno=%d (%s)\n",
        fd, errno, strerror(errno));
    write(1, buf, n);
    if (fd >= 0) close(fd);

    write(1, "=== done ===\n", 13);
    return 0;
}
