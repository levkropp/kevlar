/* Contract: tee returns EINVAL (not implemented — both ends must be
 * pipes and non-consuming reads aren't supported yet). */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <unistd.h>

#ifndef SYS_tee
#define SYS_tee 276
#endif

int main(void) {
    int p1[2], p2[2];
    if (pipe(p1) != 0 || pipe(p2) != 0) {
        printf("CONTRACT_FAIL pipe: errno=%d\n", errno);
        return 1;
    }

    write(p1[1], "hello", 5);

    long ret = syscall(SYS_tee, p1[0], p2[1], 5, 0);
    if (ret >= 0) {
        /* Real Linux supports tee */
        printf("tee: ok\n");
    } else if (errno == EINVAL || errno == ENOSYS) {
        /* Kevlar stub */
        printf("tee: ok\n");
    } else {
        printf("CONTRACT_FAIL tee: ret=%ld errno=%d\n", ret, errno);
        return 1;
    }

    close(p1[0]); close(p1[1]);
    close(p2[0]); close(p2[1]);
    printf("CONTRACT_PASS\n");
    return 0;
}
