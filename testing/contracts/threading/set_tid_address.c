/* Contract: set_tid_address returns caller's tid. */
#define _GNU_SOURCE
#include <stdio.h>
#include <sys/syscall.h>
#include <unistd.h>

int main(void) {
    int tidp;
    pid_t ret = syscall(SYS_set_tid_address, &tidp);
    pid_t tid = syscall(SYS_gettid);

    if (ret != tid) {
        printf("CONTRACT_FAIL set_tid_address: ret=%d gettid=%d\n", ret, tid);
        return 1;
    }
    printf("set_tid_address: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
