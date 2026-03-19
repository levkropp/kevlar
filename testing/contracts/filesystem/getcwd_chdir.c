/* Contract: getcwd returns current dir; chdir changes it;
 * fchdir with dirfd works. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

int main(void) {
    /* Save original cwd */
    char orig[256];
    if (getcwd(orig, sizeof(orig)) == NULL) {
        printf("CONTRACT_FAIL getcwd_init: errno=%d\n", errno);
        return 1;
    }
    printf("initial_cwd: ok\n");

    /* Create and chdir to test dir */
    const char *dir = "/tmp/contract_chdir_test";
    rmdir(dir);
    mkdir(dir, 0755);

    if (chdir(dir) != 0) {
        printf("CONTRACT_FAIL chdir: errno=%d\n", errno);
        return 1;
    }
    char cur[256];
    getcwd(cur, sizeof(cur));
    if (strcmp(cur, dir) != 0) {
        printf("CONTRACT_FAIL chdir_verify: cur=%s expected=%s\n", cur, dir);
        return 1;
    }
    printf("chdir: ok\n");

    /* fchdir back to original */
    int dfd = open(orig, O_RDONLY | O_DIRECTORY);
    if (dfd < 0) {
        printf("CONTRACT_FAIL open_orig: errno=%d\n", errno);
        return 1;
    }
    if (fchdir(dfd) != 0) {
        printf("CONTRACT_FAIL fchdir: errno=%d\n", errno);
        return 1;
    }
    close(dfd);
    getcwd(cur, sizeof(cur));
    if (strcmp(cur, orig) != 0) {
        printf("CONTRACT_FAIL fchdir_verify: cur=%s expected=%s\n", cur, orig);
        return 1;
    }
    printf("fchdir: ok\n");

    rmdir(dir);
    printf("CONTRACT_PASS\n");
    return 0;
}
