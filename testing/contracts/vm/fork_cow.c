/* Contract: fork() copy-on-write — child gets independent copy of address space.
 * Parent and child modifications must not affect each other. */
#include <stdio.h>
#include <stdlib.h>
#include <sys/wait.h>
#include <unistd.h>

static int global = 0x12345678;

int main(void) {
    pid_t pid = fork();
    if (pid < 0) {
        printf("CONTRACT_FAIL fork: %d\n", (int)pid);
        return 1;
    }
    if (pid == 0) {
        /* child: overwrite global and exit — no stdout print to avoid
         * buffering divergence between Linux pipe capture and Kevlar serial */
        global = 0xdeadbeef;
        _exit(global == 0xdeadbeef ? 0 : 1);
    }
    /* parent: wait, then check it kept original value */
    int status;
    waitpid(pid, &status, 0);
    if (global != 0x12345678) {
        printf("CONTRACT_FAIL cow: parent global=0x%x expected=0x12345678\n", global);
        return 1;
    }
    printf("parent_global: 0x%x\n", global);
    printf("CONTRACT_PASS\n");
    return 0;
}
