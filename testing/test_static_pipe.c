#include <unistd.h>
#include <sys/wait.h>
#include <string.h>
#include <stdio.h>

int main(void) {
    write(1, "=== static pipe test ===\n", 25);

    // Test: "echo hello | cat" via fork+exec
    int pfd[2];
    pipe(pfd);
    int pid1 = fork();
    if (pid1 == 0) {
        close(pfd[0]);
        dup2(pfd[1], 1);
        close(pfd[1]);
        // exec echo (busybox applet)
        char *argv[] = { "echo", "hello_from_pipe", NULL };
        execv("/bin/busybox", argv);
        _exit(1);
    }
    int pid2 = fork();
    if (pid2 == 0) {
        close(pfd[1]);
        dup2(pfd[0], 0);
        close(pfd[0]);
        // exec cat (busybox applet)
        char *argv[] = { "cat", NULL };
        execv("/bin/busybox", argv);
        _exit(1);
    }
    close(pfd[0]);
    close(pfd[1]);
    int s1, s2;
    waitpid(pid1, &s1, 0);
    waitpid(pid2, &s2, 0);
    char buf[64];
    int n = snprintf(buf, sizeof(buf), "echo: %d, cat: %d\n", s1, s2);
    write(1, buf, n);
    write(1, "=== done ===\n", 13);
    return 0;
}
