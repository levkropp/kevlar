/* Contract: MAP_SHARED visible across fork;
 * parent writes, child sees; child writes, parent sees after wait. */
#include <stdio.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

int main(void) {
    /* MAP_SHARED anonymous */
    volatile int *shared = mmap(NULL, 4096, PROT_READ | PROT_WRITE,
                                MAP_SHARED | MAP_ANONYMOUS, -1, 0);
    if (shared == MAP_FAILED) {
        printf("CONTRACT_FAIL mmap\n");
        return 1;
    }
    *shared = 0;

    pid_t pid = fork();
    if (pid == 0) {
        /* Child: verify parent's initial value, then write */
        if (*shared != 0) {
            printf("CONTRACT_FAIL child_init: val=%d\n", *shared);
            _exit(1);
        }
        *shared = 42;
        _exit(0);
    }

    /* Parent: wait for child */
    int status;
    waitpid(pid, &status, 0);
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        printf("CONTRACT_FAIL child_exit: status=0x%x\n", status);
        return 1;
    }

    /* Parent should see child's write */
    if (*shared != 42) {
        printf("CONTRACT_FAIL shared_visible: val=%d expected=42\n", *shared);
        return 1;
    }
    printf("shared_visible: ok val=%d\n", *shared);

    /* Parent writes, fork again to verify */
    *shared = 99;
    pid = fork();
    if (pid == 0) {
        if (*shared != 99) {
            printf("CONTRACT_FAIL child2_read: val=%d\n", *shared);
            _exit(1);
        }
        _exit(0);
    }
    waitpid(pid, &status, 0);
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        printf("CONTRACT_FAIL child2: status=0x%x\n", status);
        return 1;
    }
    printf("parent_write_visible: ok\n");

    munmap((void *)shared, 4096);
    printf("CONTRACT_PASS\n");
    return 0;
}
