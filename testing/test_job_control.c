// Job control tests: SIGINT, SIGTSTP, SIGCONT, kill -0.
// Tests signal delivery and process state transitions.
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

static int pass = 0, fail = 0;

int main(void) {
    printf("=== Job Control Tests ===\n");

    // Test 1: SIGINT kills child
    printf("TEST SIGINT kills child: ");
    {
        pid_t pid = fork();
        if (pid == 0) { pause(); _exit(0); } // child waits forever
        usleep(50000); // let child start
        kill(pid, SIGINT);
        int status;
        waitpid(pid, &status, 0);
        if (WIFSIGNALED(status) && WTERMSIG(status) == SIGINT) {
            printf("PASS\n"); pass++;
        } else {
            printf("FAIL (status=0x%x)\n", status); fail++;
        }
    }

    // Test 2: SIGTSTP stops child
    printf("TEST SIGTSTP stops child: ");
    {
        pid_t pid = fork();
        if (pid == 0) { pause(); _exit(0); }
        usleep(50000);
        kill(pid, SIGTSTP);
        int status;
        pid_t r = waitpid(pid, &status, WUNTRACED);
        if (r == pid && WIFSTOPPED(status) && WSTOPSIG(status) == SIGTSTP) {
            printf("PASS\n"); pass++;
        } else {
            printf("FAIL (r=%d status=0x%x)\n", r, status); fail++;
        }
        // Clean up: resume and kill
        kill(pid, SIGCONT);
        kill(pid, SIGKILL);
        waitpid(pid, NULL, 0);
    }

    // Test 3: SIGCONT resumes stopped child
    printf("TEST SIGCONT resumes stopped child: ");
    {
        pid_t pid = fork();
        if (pid == 0) {
            // Child: block SIGTSTP, use SIGSTOP instead (can't be caught)
            // Actually, just use a flag
            volatile int running = 1;
            signal(SIGTSTP, SIG_DFL);
            signal(SIGCONT, SIG_DFL);
            raise(SIGSTOP); // stop self
            // After SIGCONT, we resume here
            _exit(42);
        }
        int status;
        // Wait for child to stop
        waitpid(pid, &status, WUNTRACED);
        if (!WIFSTOPPED(status)) {
            printf("FAIL (child didn't stop)\n"); fail++;
        } else {
            // Resume it
            kill(pid, SIGCONT);
            waitpid(pid, &status, 0);
            if (WIFEXITED(status) && WEXITSTATUS(status) == 42) {
                printf("PASS\n"); pass++;
            } else {
                printf("FAIL (after resume: status=0x%x)\n", status); fail++;
            }
        }
    }

    // Test 4: kill -0 checks process existence
    printf("TEST kill -0 (process exists): ");
    {
        pid_t pid = fork();
        if (pid == 0) { sleep(5); _exit(0); }
        if (kill(pid, 0) == 0) {
            printf("PASS\n"); pass++;
        } else {
            printf("FAIL (errno=%d)\n", errno); fail++;
        }
        kill(pid, SIGKILL);
        waitpid(pid, NULL, 0);
    }

    // Test 5: kill -0 on dead process returns ESRCH
    printf("TEST kill -0 (dead process): ");
    {
        pid_t pid = fork();
        if (pid == 0) { _exit(0); }
        waitpid(pid, NULL, 0); // reap
        int r = kill(pid, 0);
        if (r == -1 && errno == ESRCH) {
            printf("PASS\n"); pass++;
        } else {
            printf("FAIL (r=%d errno=%d)\n", r, errno); fail++;
        }
    }

    // Test 6: SIGKILL is not catchable
    printf("TEST SIGKILL kills unconditionally: ");
    {
        pid_t pid = fork();
        if (pid == 0) {
            signal(SIGKILL, SIG_IGN); // attempt to ignore (should fail)
            pause();
            _exit(0);
        }
        usleep(50000);
        kill(pid, SIGKILL);
        int status;
        waitpid(pid, &status, 0);
        if (WIFSIGNALED(status) && WTERMSIG(status) == SIGKILL) {
            printf("PASS\n"); pass++;
        } else {
            printf("FAIL (status=0x%x)\n", status); fail++;
        }
    }

    // Test 7: waitpid with WNOHANG
    printf("TEST waitpid WNOHANG: ");
    {
        pid_t pid = fork();
        if (pid == 0) { sleep(5); _exit(0); }
        int status;
        pid_t r = waitpid(pid, &status, WNOHANG);
        if (r == 0) {
            printf("PASS (child still running)\n"); pass++;
        } else {
            printf("FAIL (r=%d)\n", r); fail++;
        }
        kill(pid, SIGKILL);
        waitpid(pid, NULL, 0);
    }

    printf("\n=== Results: %d PASS, %d FAIL ===\n", pass, fail);
    printf(fail == 0 ? "TEST_PASS\n" : "TEST_FAIL\n");
    return fail > 0 ? 1 : 0;
}
