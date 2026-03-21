/* Contract: pidfd_open returns a valid fd on Linux; on Kevlar it returns
 * ENOSYS (stub — systemd falls back to SIGCHLD). Either is acceptable. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <unistd.h>

#ifndef SYS_pidfd_open
#define SYS_pidfd_open 434
#endif

int main(void) {
    pid_t pid = getpid();

    long ret = syscall(SYS_pidfd_open, pid, 0);
    if (ret >= 0) {
        /* Real Linux: got a pidfd */
        printf("pidfd_open: ok\n");
        close((int)ret);
    } else if (errno == ENOSYS) {
        /* Kevlar stub: ENOSYS */
        printf("pidfd_open: ok\n");
    } else {
        printf("CONTRACT_FAIL pidfd_open: ret=%ld errno=%d\n", ret, errno);
        return 1;
    }

    /* Invalid PID should return error (ESRCH or ENOSYS) */
    errno = 0;
    ret = syscall(SYS_pidfd_open, -1, 0);
    if (ret >= 0) {
        printf("CONTRACT_FAIL bad_pid_accepted: ret=%ld\n", ret);
        close((int)ret);
        return 1;
    }
    /* ESRCH on Linux, ENOSYS on Kevlar — both acceptable */
    if (errno != ESRCH && errno != EINVAL && errno != ENOSYS) {
        printf("CONTRACT_FAIL bad_pid: errno=%d\n", errno);
        return 1;
    }
    printf("bad_pid: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
