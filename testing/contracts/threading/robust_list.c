/* Contract: set_robust_list accepts valid args; rejects invalid size. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <unistd.h>

struct robust_list_head {
    void *list;
    long futex_offset;
    void *list_op_pending;
};

int main(void) {
    struct robust_list_head head = {0};
    head.list = &head; /* point to self = empty list */

    /* Valid: size = sizeof(struct robust_list_head) = 24 on x86_64 */
    int ret = syscall(SYS_set_robust_list, &head, sizeof(head));
    if (ret != 0) {
        printf("CONTRACT_FAIL valid: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("valid: ok\n");

    /* Invalid size → EINVAL */
    errno = 0;
    ret = syscall(SYS_set_robust_list, &head, 999);
    if (ret != -1 || errno != EINVAL) {
        printf("CONTRACT_FAIL bad_size: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("bad_size: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
