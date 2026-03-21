#include <fcntl.h>
#include <unistd.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <string.h>
#include <stdio.h>
#include <errno.h>

int main(void) {
    write(1, "=== apk trace test ===\n", 23);
    mkdir("/mnt", 0755);
    mount("tmpfs", "/mnt", "tmpfs", 0, NULL);
    mkdir("/mnt/r", 0755);
    mount("none", "/mnt/r", "ext4", 0, NULL);
    mkdir("/mnt/r/proc", 0755);
    mount("proc", "/mnt/r/proc", "proc", 0, NULL);
    mkdir("/mnt/r/dev", 0755);
    mount("devtmpfs", "/mnt/r/dev", "devtmpfs", 0, NULL);
    mkdir("/mnt/r/run", 0755);
    mount("tmpfs", "/mnt/r/run", "tmpfs", 0, NULL);
    mkdir("/mnt/r/tmp", 01777);
    mount("tmpfs", "/mnt/r/tmp", "tmpfs", 0, NULL);
    mkdir("/mnt/r/old", 0755);
    syscall(155, "/mnt/r", "/mnt/r/old");
    chdir("/");

    // Set up networking
    // (ip commands via fork+exec would be slow, use ioctl instead)
    // Actually, apk update needs network. Let me use the netlink approach:
    write(1, "configuring network...\n", 22);
    int pid = fork();
    if (pid == 0) {
        char *argv[] = { "ip", "link", "set", "eth0", "up", NULL };
        char *envp[] = { "PATH=/sbin:/bin", NULL };
        execve("/sbin/ip", argv, envp);
        _exit(1);
    }
    waitpid(pid, NULL, 0);
    pid = fork();
    if (pid == 0) {
        char *argv[] = { "ip", "addr", "add", "10.0.2.15/24", "dev", "eth0", NULL };
        char *envp[] = { "PATH=/sbin:/bin", NULL };
        execve("/sbin/ip", argv, envp);
        _exit(1);
    }
    waitpid(pid, NULL, 0);
    pid = fork();
    if (pid == 0) {
        char *argv[] = { "ip", "route", "add", "default", "via", "10.0.2.2", NULL };
        char *envp[] = { "PATH=/sbin:/bin", NULL };
        execve("/sbin/ip", argv, envp);
        _exit(1);
    }
    waitpid(pid, NULL, 0);
    write(1, "network ready\n", 14);

    // Now run apk update
    write(1, "running apk update...\n", 22);
    pid = fork();
    if (pid == 0) {
        char *argv[] = { "apk", "update", "--no-progress", NULL };
        char *envp[] = { "PATH=/usr/sbin:/usr/bin:/sbin:/bin", NULL };
        execve("/sbin/apk", argv, envp);
        _exit(1);
    }
    int status;
    waitpid(pid, &status, 0);
    char buf[64];
    int n = snprintf(buf, sizeof(buf), "apk status: 0x%x\n", status);
    write(1, buf, n);

    write(1, "=== done ===\n", 13);
    return 0;
}
