/* Contract: prctl PR_SET_NAME/PR_GET_NAME and PR_SET/GET_CHILD_SUBREAPER. */
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/prctl.h>
#include <unistd.h>

int main(void) {
    /* PR_SET_NAME / PR_GET_NAME */
    if (prctl(PR_SET_NAME, "mythread", 0, 0, 0) != 0) {
        printf("CONTRACT_FAIL set_name: errno=%d\n", errno);
        return 1;
    }
    char name[16] = {0};
    if (prctl(PR_GET_NAME, name, 0, 0, 0) != 0) {
        printf("CONTRACT_FAIL get_name: errno=%d\n", errno);
        return 1;
    }
    if (strcmp(name, "mythread") != 0) {
        printf("CONTRACT_FAIL name_mismatch: got='%s'\n", name);
        return 1;
    }
    printf("prctl_name: ok name='%s'\n", name);

    /* PR_SET_CHILD_SUBREAPER 1 → PR_GET reads 1 */
    if (prctl(PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) != 0) {
        printf("CONTRACT_FAIL set_subreaper: errno=%d\n", errno);
        return 1;
    }
    int val = 0;
    if (prctl(PR_GET_CHILD_SUBREAPER, (unsigned long)&val, 0, 0, 0) != 0) {
        printf("CONTRACT_FAIL get_subreaper: errno=%d\n", errno);
        return 1;
    }
    if (val != 1) {
        printf("CONTRACT_FAIL subreaper_on: val=%d\n", val);
        return 1;
    }
    printf("subreaper_on: ok val=%d\n", val);

    /* PR_SET_CHILD_SUBREAPER 0 → PR_GET reads 0 */
    if (prctl(PR_SET_CHILD_SUBREAPER, 0, 0, 0, 0) != 0) {
        printf("CONTRACT_FAIL unset_subreaper: errno=%d\n", errno);
        return 1;
    }
    val = 99;
    if (prctl(PR_GET_CHILD_SUBREAPER, (unsigned long)&val, 0, 0, 0) != 0) {
        printf("CONTRACT_FAIL get_subreaper2: errno=%d\n", errno);
        return 1;
    }
    if (val != 0) {
        printf("CONTRACT_FAIL subreaper_off: val=%d\n", val);
        return 1;
    }
    printf("subreaper_off: ok val=%d\n", val);

    printf("CONTRACT_PASS\n");
    return 0;
}
