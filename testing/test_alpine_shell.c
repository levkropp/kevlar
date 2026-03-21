// Reproduce the Alpine pipe crash: exec dynamic /bin/sh -c "ls | wc -l"
// This is exactly what BusyBox init does for the pipe test inittab line.
#include <unistd.h>
#include <sys/wait.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <string.h>
#include <stdio.h>

int main(void) {
    write(1, "=== alpine shell pipe test ===\n", 31);

    // Set up Alpine chroot
    mkdir("/mnt", 0755);
    mount("tmpfs", "/mnt", "tmpfs", 0, NULL);
    mkdir("/mnt/root", 0755);
    mount("none", "/mnt/root", "ext4", 0, NULL);

    // Test 1: exec dynamic shell with simple command (no pipe)
    write(1, "test1: sh -c 'echo nopipe'\n", 27);
    int pid = fork();
    if (pid == 0) {
        chroot("/mnt/root");
        chdir("/");
        char *argv[] = { "sh", "-c", "echo nopipe_ok", NULL };
        char *envp[] = { "PATH=/bin:/usr/bin", "HOME=/root", NULL };
        execve("/bin/sh", argv, envp);
        _exit(1);
    }
    int status;
    waitpid(pid, &status, 0);
    char buf[64];
    int n = snprintf(buf, sizeof(buf), "test1 status: 0x%x\n", status);
    write(1, buf, n);

    // Test 2: exec dynamic shell with pipe command
    write(1, "test2: sh -c 'echo hello | cat'\n", 32);
    pid = fork();
    if (pid == 0) {
        chroot("/mnt/root");
        chdir("/");
        char *argv[] = { "sh", "-c", "echo hello | cat", NULL };
        char *envp[] = { "PATH=/bin:/usr/bin", "HOME=/root", NULL };
        execve("/bin/sh", argv, envp);
        _exit(1);
    }
    waitpid(pid, &status, 0);
    n = snprintf(buf, sizeof(buf), "test2 status: 0x%x\n", status);
    write(1, buf, n);

    // Test 3: exec dynamic shell with ls | head
    write(1, "test3: sh -c 'ls / | head -3'\n", 30);
    pid = fork();
    if (pid == 0) {
        chroot("/mnt/root");
        chdir("/");
        char *argv[] = { "sh", "-c", "ls / | head -3", NULL };
        char *envp[] = { "PATH=/bin:/usr/bin", "HOME=/root", NULL };
        execve("/bin/sh", argv, envp);
        _exit(1);
    }
    waitpid(pid, &status, 0);
    n = snprintf(buf, sizeof(buf), "test3 status: 0x%x\n", status);
    write(1, buf, n);

    write(1, "=== done ===\n", 13);
    return 0;
}
