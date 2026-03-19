/* Contract: mkdir/rmdir create and remove directories;
 * EEXIST, ENOTEMPTY, ENOENT error codes. */
#include <errno.h>
#include <stdio.h>
#include <sys/stat.h>
#include <unistd.h>

int main(void) {
    const char *dir = "/tmp/contract_dir";
    rmdir(dir); /* cleanup */

    /* mkdir succeeds */
    if (mkdir(dir, 0755) != 0) {
        printf("CONTRACT_FAIL mkdir: errno=%d\n", errno);
        return 1;
    }

    /* Verify it exists with stat */
    struct stat st;
    if (stat(dir, &st) != 0 || !S_ISDIR(st.st_mode)) {
        printf("CONTRACT_FAIL stat_dir: errno=%d mode=0x%x\n", errno, st.st_mode);
        return 1;
    }
    printf("mkdir: ok\n");

    /* Duplicate mkdir → EEXIST */
    errno = 0;
    if (mkdir(dir, 0755) != -1 || errno != EEXIST) {
        printf("CONTRACT_FAIL eexist: errno=%d\n", errno);
        return 1;
    }
    printf("eexist: ok\n");

    /* rmdir on non-empty → ENOTEMPTY */
    const char *child = "/tmp/contract_dir/sub";
    mkdir(child, 0755);
    errno = 0;
    if (rmdir(dir) != -1 || errno != ENOTEMPTY) {
        printf("CONTRACT_FAIL enotempty: errno=%d\n", errno);
        return 1;
    }
    printf("enotempty: ok\n");

    /* rmdir child then parent */
    rmdir(child);
    if (rmdir(dir) != 0) {
        printf("CONTRACT_FAIL rmdir: errno=%d\n", errno);
        return 1;
    }
    printf("rmdir: ok\n");

    /* rmdir on missing → ENOENT */
    errno = 0;
    if (rmdir(dir) != -1 || errno != ENOENT) {
        printf("CONTRACT_FAIL enoent: errno=%d\n", errno);
        return 1;
    }
    printf("enoent: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
