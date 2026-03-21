// Reproduce pipe crash with extra diagnostics.
// After fork in the shell, the child tries to access musl data.
// Check if we can access musl's nl_langinfo before the shell does.
#include <unistd.h>
#include <sys/wait.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <string.h>
#include <stdio.h>
#include <langinfo.h>  // nl_langinfo

int main(void) {
    write(1, "=== ash pipe diag ===\n", 22);

    mkdir("/mnt", 0755);
    mount("tmpfs", "/mnt", "tmpfs", 0, NULL);
    mkdir("/mnt/root", 0755);
    mount("none", "/mnt/root", "ext4", 0, NULL);

    // Fork and exec the dynamic shell, but first have the child test nl_langinfo
    int pid = fork();
    if (pid == 0) {
        chroot("/mnt/root");
        chdir("/");
        // Exec dynamic busybox "echo" — this tests the dynamic linker
        char *argv[] = { "sh", "-c", "echo hello | cat", NULL };
        char *envp[] = { "PATH=/bin:/usr/bin", "HOME=/root", "LANG=C", "LC_ALL=C", NULL };
        execve("/bin/sh", argv, envp);
        _exit(1);
    }

    int status;
    waitpid(pid, &status, 0);
    char buf[64];
    int n = snprintf(buf, sizeof(buf), "status: 0x%x (sig=%d)\n", status, status & 0x7f);
    write(1, buf, n);

    // Now try the same but with explicit fork+exec of just echo (no pipe)
    write(1, "test2: fork+exec dynamic echo\n", 30);
    pid = fork();
    if (pid == 0) {
        chroot("/mnt/root");
        chdir("/");
        char *argv[] = { "echo", "direct_exec_ok", NULL };
        char *envp[] = { "PATH=/bin", "LANG=C", NULL };
        execve("/bin/busybox", argv, envp);
        _exit(1);
    }
    waitpid(pid, &status, 0);
    n = snprintf(buf, sizeof(buf), "status: 0x%x\n", status);
    write(1, buf, n);

    // Test 3: the SHELL forks internally. Does it work if we don't use a pipe?
    write(1, "test3: shell subshell (no pipe)\n", 31);
    pid = fork();
    if (pid == 0) {
        chroot("/mnt/root");
        chdir("/");
        char *argv[] = { "sh", "-c", "echo subshell_ok", NULL };
        char *envp[] = { "PATH=/bin", "LANG=C", NULL };
        execve("/bin/sh", argv, envp);
        _exit(1);
    }
    waitpid(pid, &status, 0);
    n = snprintf(buf, sizeof(buf), "status: 0x%x\n", status);
    write(1, buf, n);

    // Test 4: shell with backtick (also forks internally)
    write(1, "test4: shell with $(echo foo)\n", 29);
    pid = fork();
    if (pid == 0) {
        chroot("/mnt/root");
        chdir("/");
        char *argv[] = { "sh", "-c", "echo $(echo foo)", NULL };
        char *envp[] = { "PATH=/bin", "LANG=C", NULL };
        execve("/bin/sh", argv, envp);
        _exit(1);
    }
    waitpid(pid, &status, 0);
    n = snprintf(buf, sizeof(buf), "status: 0x%x\n", status);
    write(1, buf, n);

    write(1, "=== done ===\n", 13);
    return 0;
}
