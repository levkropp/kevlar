#include <unistd.h>
#include <sys/wait.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <string.h>
#include <stdio.h>

int main(void) {
    write(1, "=== dynamic pipe test ===\n", 26);

    // Mount Alpine rootfs
    mkdir("/mnt", 0755);
    mount("tmpfs", "/mnt", "tmpfs", 0, NULL);
    mkdir("/mnt/root", 0755);
    mount("none", "/mnt/root", "ext4", 0, NULL);

    // Test: fork+exec dynamic busybox "echo" piped to dynamic busybox "cat"
    int pfd[2];
    pipe(pfd);

    int pid1 = fork();
    if (pid1 == 0) {
        // Writer: echo
        close(pfd[0]);
        dup2(pfd[1], 1);
        close(pfd[1]);
        chroot("/mnt/root");
        chdir("/");
        char *argv[] = { "echo", "piped_dynamic", NULL };
        char *envp[] = { "PATH=/bin", NULL };
        execve("/bin/busybox", argv, envp);
        _exit(1);
    }

    int pid2 = fork();
    if (pid2 == 0) {
        // Reader: cat
        close(pfd[1]);
        dup2(pfd[0], 0);
        close(pfd[0]);
        chroot("/mnt/root");
        chdir("/");
        char *argv[] = { "cat", NULL };
        char *envp[] = { "PATH=/bin", NULL };
        execve("/bin/busybox", argv, envp);
        _exit(1);
    }

    close(pfd[0]);
    close(pfd[1]);

    int s1, s2;
    waitpid(pid1, &s1, 0);
    waitpid(pid2, &s2, 0);
    char buf[96];
    int n = snprintf(buf, sizeof(buf), "echo: 0x%x, cat: 0x%x\n", s1, s2);
    write(1, buf, n);
    write(1, "=== done ===\n", 13);
    return 0;
}
