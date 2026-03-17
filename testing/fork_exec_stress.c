// Fork+exec stress test for huge page assembly verification.
// Runs ITERATIONS fork+exec+wait cycles of /bin/true, checking each exit status.
// Usage: fork_exec_stress [iterations]

#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <sys/wait.h>

#define DEFAULT_ITERATIONS 300

int main(int argc, char *argv[]) {
    int iterations = DEFAULT_ITERATIONS;
    if (argc > 1)
        iterations = atoi(argv[1]);
    if (iterations <= 0)
        iterations = DEFAULT_ITERATIONS;

    int pass = 0, fail = 0;
    for (int i = 0; i < iterations; i++) {
        pid_t pid = fork();
        if (pid < 0) {
            printf("TEST_FAIL fork_exec_stress iter %d: fork failed\n", i);
            fail++;
            continue;
        }
        if (pid == 0) {
            execl("/bin/true", "true", (char *)NULL);
            _exit(127);
        }
        int status;
        if (waitpid(pid, &status, 0) < 0) {
            printf("TEST_FAIL fork_exec_stress iter %d: waitpid failed\n", i);
            fail++;
            continue;
        }
        if (WIFSIGNALED(status)) {
            printf("TEST_FAIL fork_exec_stress iter %d: killed by signal %d\n",
                   i, WTERMSIG(status));
            fail++;
        } else if (WEXITSTATUS(status) != 0) {
            printf("TEST_FAIL fork_exec_stress iter %d: exit status %d\n",
                   i, WEXITSTATUS(status));
            fail++;
        } else {
            pass++;
        }

        // Early abort after 5 failures to avoid flooding output.
        if (fail >= 5) {
            printf("TEST_FAIL fork_exec_stress: aborting after %d failures\n", fail);
            break;
        }
    }

    if (fail == 0)
        printf("TEST_PASS fork_exec_stress (%d/%d iterations)\n", pass, iterations);
    else
        printf("TEST_FAIL fork_exec_stress (%d pass, %d fail)\n", pass, fail);

    return fail > 0 ? 1 : 0;
}
