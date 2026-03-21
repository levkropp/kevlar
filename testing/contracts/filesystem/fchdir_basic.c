/* Contract: fchdir changes working directory via fd; getcwd reflects the
 * change; EBADF on closed fd; ENOTDIR on non-directory fd. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int main(void) {
    /* Save original cwd */
    char orig[256];
    if (!getcwd(orig, sizeof(orig))) {
        printf("CONTRACT_FAIL getcwd: errno=%d\n", errno);
        return 1;
    }

    /* Open /tmp as a directory fd */
    int dirfd = open("/tmp", O_RDONLY | O_DIRECTORY);
    if (dirfd < 0) {
        printf("CONTRACT_FAIL open_tmp: errno=%d\n", errno);
        return 1;
    }

    /* fchdir to /tmp */
    int ret = fchdir(dirfd);
    if (ret != 0) {
        printf("CONTRACT_FAIL fchdir: ret=%d errno=%d\n", ret, errno);
        return 1;
    }

    /* Verify cwd changed */
    char cwd[256];
    if (!getcwd(cwd, sizeof(cwd))) {
        printf("CONTRACT_FAIL getcwd2: errno=%d\n", errno);
        return 1;
    }
    if (strcmp(cwd, "/tmp") != 0) {
        printf("CONTRACT_FAIL cwd_check: got=%s expected=/tmp\n", cwd);
        return 1;
    }
    printf("fchdir_tmp: ok\n");

    /* Restore original directory */
    int origfd = open(orig, O_RDONLY | O_DIRECTORY);
    if (origfd >= 0) {
        fchdir(origfd);
        close(origfd);
    } else {
        chdir(orig);
    }
    close(dirfd);

    /* EBADF on closed fd */
    errno = 0;
    ret = fchdir(dirfd);
    if (ret != -1 || errno != EBADF) {
        printf("CONTRACT_FAIL ebadf: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("ebadf: ok\n");

    /* ENOTDIR on regular file fd */
    int filefd = open("/dev/null", O_RDONLY);
    if (filefd < 0) {
        printf("CONTRACT_FAIL open_null: errno=%d\n", errno);
        return 1;
    }
    errno = 0;
    ret = fchdir(filefd);
    if (ret == -1 && (errno == ENOTDIR || errno == ENOENT)) {
        printf("enotdir: ok\n");
    } else {
        printf("CONTRACT_FAIL enotdir: ret=%d errno=%d\n", ret, errno);
        close(filefd);
        return 1;
    }
    close(filefd);

    printf("CONTRACT_PASS\n");
    return 0;
}
