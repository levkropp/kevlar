#include <unistd.h>
#include <sys/wait.h>
#include <string.h>
#include <stdio.h>
#include <sys/mount.h>
#include <sys/stat.h>

int main(void) {
    write(1, "=== dynamic exec test ===\n", 26);

    // Mount ext4 to access Alpine's dynamic busybox
    mkdir("/mnt", 0755);
    mount("tmpfs", "/mnt", "tmpfs", 0, NULL);
    mkdir("/mnt/root", 0755);
    mount("none", "/mnt/root", "ext4", 0, NULL);

    // Fork and exec Alpine's dynamic busybox (echo applet)
    int pid = fork();
    if (pid == 0) {
        // Child: chroot into Alpine and exec dynamic busybox
        chroot("/mnt/root");
        chdir("/");
        char *argv[] = { "echo", "dynamic_exec_works", NULL };
        char *envp[] = { "PATH=/bin:/sbin:/usr/bin", NULL };
        execve("/bin/busybox", argv, envp);
        write(2, "execve failed\n", 14);
        _exit(1);
    }

    int status;
    waitpid(pid, &status, 0);
    char buf[64];
    int n = snprintf(buf, sizeof(buf), "child status: 0x%x\n", status);
    write(1, buf, n);

    write(1, "=== done ===\n", 13);
    return 0;
}
