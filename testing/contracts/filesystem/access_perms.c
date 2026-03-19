/* Contract: access/faccessat check file existence and permissions. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <unistd.h>

int main(void) {
    /* F_OK on existing file */
    if (access("/dev/null", F_OK) != 0) {
        printf("CONTRACT_FAIL dev_null_exists: errno=%d\n", errno);
        return 1;
    }
    printf("dev_null_exists: ok\n");

    /* F_OK on missing → ENOENT */
    errno = 0;
    if (access("/nonexistent_path_12345", F_OK) != -1 || errno != ENOENT) {
        printf("CONTRACT_FAIL missing_enoent: errno=%d\n", errno);
        return 1;
    }
    printf("missing_enoent: ok\n");

    /* R_OK + W_OK on /dev/null */
    if (access("/dev/null", R_OK | W_OK) != 0) {
        printf("CONTRACT_FAIL dev_null_rw: errno=%d\n", errno);
        return 1;
    }
    printf("dev_null_rw: ok\n");

    /* faccessat with AT_FDCWD */
    if (faccessat(AT_FDCWD, "/dev/null", F_OK, 0) != 0) {
        printf("CONTRACT_FAIL faccessat: errno=%d\n", errno);
        return 1;
    }
    printf("faccessat: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
