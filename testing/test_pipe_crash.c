#include <unistd.h>
#include <string.h>
#include <sys/wait.h>
#include <stdio.h>

int main(void) {
    write(1, "pipe test start\n", 16);

    int pfd[2];
    if (pipe(pfd) != 0) {
        write(1, "pipe() failed\n", 14);
        return 1;
    }

    int pid = fork();
    if (pid == 0) {
        // Child: write to pipe
        close(pfd[0]);
        dup2(pfd[1], 1);
        close(pfd[1]);
        // Write a lot
        char buf[256];
        for (int i = 0; i < 100; i++) {
            int n = snprintf(buf, sizeof(buf), "line %d of output\n", i);
            int r = write(1, buf, n);
            if (r < 0) break;
        }
        _exit(0);
    }

    // Parent: read one line from pipe, then close
    close(pfd[1]);
    char buf[256];
    int n = read(pfd[0], buf, sizeof(buf));
    if (n > 0) {
        write(2, "read: ", 6);
        write(2, buf, n > 64 ? 64 : n);
    }
    close(pfd[0]); // This triggers SIGPIPE/EPIPE in the child

    int status;
    waitpid(pid, &status, 0);
    char msg[64];
    int mlen = snprintf(msg, sizeof(msg), "child status: %d\n", status);
    write(1, msg, mlen);
    write(1, "pipe test done\n", 15);
    return 0;
}
