// Minimal reproducer for OpenRC's self-pipe SIGCHLD pattern.
// Parent: creates pipe, installs SIGCHLD handler, forks child,
// child execs hostname, parent polls pipe for POLLIN.
// On Linux: poll returns when SIGCHLD handler writes to pipe.
//
// Build: musl-gcc -static -O2 -o test-sigchld-pipe test_sigchld_pipe.c
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

static int signal_pipe[2] = {-1, -1};
static pid_t child_pid = -1;

static void sigchld_handler(int sig) {
    (void)sig;
    int status;
    pid_t pid;
    while ((pid = waitpid(-1, &status, WNOHANG)) > 0) {
        if (pid == child_pid && signal_pipe[1] >= 0) {
            int w = write(signal_pipe[1], &status, sizeof(status));
            (void)w;
        }
    }
}

static void msg(const char *s) { write(1, s, strlen(s)); }

int main(void) {
    msg("=== SIGCHLD self-pipe test ===\n");

    // Create pipe with manual CLOEXEC (matching OpenRC's pattern)
    if (pipe(signal_pipe) == -1) {
        msg("TEST_FAIL pipe\n");
        return 1;
    }
    for (int i = 0; i < 2; i++) {
        int flags = fcntl(signal_pipe[i], F_GETFD, 0);
        fcntl(signal_pipe[i], F_SETFD, flags | FD_CLOEXEC);
    }
    char buf[128];
    snprintf(buf, sizeof(buf), "DIAG: pipe fds: read=%d write=%d\n",
             signal_pipe[0], signal_pipe[1]);
    msg(buf);

    // Install SIGCHLD handler (match OpenRC: no SA_RESTART, no SA_NOCLDSTOP)
    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = sigchld_handler;
    sa.sa_flags = 0;  // OpenRC uses signal_setup() which has no flags
    sigemptyset(&sa.sa_mask);
    if (sigaction(SIGCHLD, &sa, NULL) == -1) {
        msg("TEST_FAIL sigaction\n");
        return 1;
    }
    msg("DIAG: SIGCHLD handler installed\n");

    // Fork child
    child_pid = fork();
    if (child_pid == -1) {
        msg("TEST_FAIL fork\n");
        return 1;
    }

    if (child_pid == 0) {
        // Child: exec a shell command (matching OpenRC's pattern of exec shell script)
        char *argv[] = {"/bin/sh", "-c", "echo CHILD_HELLO; hostname test-host 2>/dev/null; exit 0", NULL};
        execv(argv[0], argv);
        _exit(127);
    }

    snprintf(buf, sizeof(buf), "DIAG: forked child pid=%d\n", child_pid);
    msg(buf);

    // Parent: poll signal_pipe[0] for POLLIN (SIGCHLD handler writes status)
    struct pollfd pfd;
    pfd.fd = signal_pipe[0];
    pfd.events = POLLIN;
    pfd.revents = 0;

    msg("DIAG: entering poll(-1)...\n");
    int ret;
    for (int attempt = 0; attempt < 5; attempt++) {
        ret = poll(&pfd, 1, 5000); // 5 second timeout per attempt
        snprintf(buf, sizeof(buf),
                 "DIAG: poll returned %d, revents=%#x, errno=%d\n",
                 ret, pfd.revents, ret < 0 ? errno : 0);
        msg(buf);

        if (ret > 0 && (pfd.revents & POLLIN)) {
            // Read the status from the pipe
            int status;
            int n = read(signal_pipe[0], &status, sizeof(status));
            snprintf(buf, sizeof(buf),
                     "DIAG: read %d bytes from pipe, status=%d\n", n, status);
            msg(buf);
            if (n == sizeof(status)) {
                msg("TEST_PASS sigchld_pipe\n");
                return 0;
            }
        }

        if (ret == 0) {
            msg("DIAG: poll timeout, retrying...\n");
            // Try non-blocking waitpid to see if child exited
            int status;
            pid_t wp = waitpid(child_pid, &status, WNOHANG);
            snprintf(buf, sizeof(buf),
                     "DIAG: waitpid(WNOHANG) returned %d, status=%d\n",
                     (int)wp, status);
            msg(buf);
        }

        if (ret < 0 && errno == EINTR) {
            msg("DIAG: poll EINTR (signal), retrying...\n");
            continue;
        }
    }

    msg("TEST_FAIL sigchld_pipe (poll never got POLLIN)\n");
    return 1;
}
