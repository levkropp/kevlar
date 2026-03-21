/* Contract: getpid/getppid/gettid return consistent values; gettid
 * equals getpid for single-threaded process; getppid is valid. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <unistd.h>

int main(void) {
    pid_t pid = getpid();
    if (pid <= 0) {
        printf("CONTRACT_FAIL getpid: pid=%d\n", pid);
        return 1;
    }
    printf("getpid: ok\n");

    pid_t ppid = getppid();
    if (ppid < 0) {
        printf("CONTRACT_FAIL getppid: ppid=%d\n", ppid);
        return 1;
    }
    printf("getppid: ok\n");

    pid_t tid = syscall(SYS_gettid);
    if (tid <= 0) {
        printf("CONTRACT_FAIL gettid: tid=%d\n", tid);
        return 1;
    }

    /* In a single-threaded process, tid == pid */
    if (tid != pid) {
        printf("CONTRACT_FAIL tid_eq_pid: tid=%d pid=%d\n", tid, pid);
        return 1;
    }
    printf("gettid: ok\n");

    /* getpid is stable across calls */
    if (getpid() != pid) {
        printf("CONTRACT_FAIL pid_stable\n");
        return 1;
    }
    printf("pid_stable: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
