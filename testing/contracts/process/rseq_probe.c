/* Contract: rseq returns ENOSYS for valid-looking args (glibc probes
 * this); returns EINVAL for bad length. Either behavior is acceptable. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <unistd.h>

#ifndef SYS_rseq
#define SYS_rseq 334
#endif

int main(void) {
    /* Valid-looking rseq registration: 32 bytes, sig=0 */
    char rseq_area[32];
    __builtin_memset(rseq_area, 0, sizeof(rseq_area));

    long ret = syscall(SYS_rseq, rseq_area, sizeof(rseq_area), 0, 0);
    if (ret == 0) {
        /* Real Linux registered rseq */
        printf("rseq_register: ok\n");
        /* Unregister */
        syscall(SYS_rseq, rseq_area, sizeof(rseq_area), 1, 0);
    } else if (errno == ENOSYS) {
        printf("rseq_register: ok\n");
    } else if (errno == EINVAL) {
        /* Some configurations reject it */
        printf("rseq_register: ok\n");
    } else {
        printf("CONTRACT_FAIL rseq: ret=%ld errno=%d\n", ret, errno);
        return 1;
    }

    /* Bad length should get EINVAL (not crash) */
    errno = 0;
    ret = syscall(SYS_rseq, rseq_area, 4, 0, 0);
    if (ret == -1 && (errno == EINVAL || errno == ENOSYS)) {
        printf("rseq_bad_len: ok\n");
    } else {
        printf("CONTRACT_FAIL bad_len: ret=%ld errno=%d\n", ret, errno);
        return 1;
    }

    printf("CONTRACT_PASS\n");
    return 0;
}
