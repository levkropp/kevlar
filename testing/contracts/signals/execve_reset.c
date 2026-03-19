/* Contract: execve resets caught signal handlers to SIG_DFL;
 * SIG_IGN survives execve. Self-re-exec pattern with argv flag. */
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

static void noop_handler(int sig) { (void)sig; }

int main(int argc, char *argv[]) {
    if (argc > 1 && strcmp(argv[1], "--child") == 0) {
        /* Re-exec'd child: check that handlers are reset */
        struct sigaction sa;

        /* SIGUSR1 was caught → should be SIG_DFL after execve */
        sigaction(SIGUSR1, NULL, &sa);
        if (sa.sa_handler != SIG_DFL) {
            printf("CONTRACT_FAIL usr1_reset: handler=%p\n", (void *)(long)sa.sa_handler);
            return 1;
        }
        printf("usr1_reset: ok\n");

        /* SIGUSR2 was SIG_IGN → should survive execve */
        sigaction(SIGUSR2, NULL, &sa);
        if (sa.sa_handler != SIG_IGN) {
            printf("CONTRACT_FAIL usr2_ign: handler=%p\n", (void *)(long)sa.sa_handler);
            return 1;
        }
        printf("usr2_ign_survives: ok\n");

        printf("CONTRACT_PASS\n");
        return 0;
    }

    /* Parent: set up handlers then fork+exec self with --child */
    struct sigaction sa = {0};
    sa.sa_handler = noop_handler;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGUSR1, &sa, NULL);

    sa.sa_handler = SIG_IGN;
    sigaction(SIGUSR2, &sa, NULL);

    pid_t pid = fork();
    if (pid == 0) {
        /* Child: exec self with --child */
        char *args[] = {argv[0], "--child", NULL};
        execv(argv[0], args);
        printf("CONTRACT_FAIL execv: should not reach here\n");
        _exit(1);
    }

    int status;
    waitpid(pid, &status, 0);
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        printf("CONTRACT_FAIL child_status: %d\n", status);
        return 1;
    }
    /* Child already printed CONTRACT_PASS if it reached it */
    return 0;
}
