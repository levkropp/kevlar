/* Contract: chmod changes permissions by path; chown changes owner by
 * path; stat reflects both changes. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <sys/stat.h>
#include <unistd.h>

int main(void) {
    /* Create test file */
    int fd = open("/tmp/chmod_test", O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (fd < 0) {
        printf("CONTRACT_FAIL open: errno=%d\n", errno);
        return 1;
    }
    close(fd);

    /* chmod to 0755 */
    int ret = chmod("/tmp/chmod_test", 0755);
    if (ret != 0) {
        printf("CONTRACT_FAIL chmod: errno=%d\n", errno);
        return 1;
    }
    struct stat st;
    stat("/tmp/chmod_test", &st);
    if ((st.st_mode & 0777) != 0755) {
        printf("CONTRACT_FAIL chmod_verify: mode=0%03o\n", st.st_mode & 0777);
        return 1;
    }
    printf("chmod: ok\n");

    /* chmod to 0600 */
    chmod("/tmp/chmod_test", 0600);
    stat("/tmp/chmod_test", &st);
    if ((st.st_mode & 0777) != 0600) {
        printf("CONTRACT_FAIL chmod2: mode=0%03o\n", st.st_mode & 0777);
        return 1;
    }
    printf("chmod_restrictive: ok\n");

    /* chown to own uid/gid (always permitted) */
    uid_t myuid = getuid();
    gid_t mygid = getgid();
    ret = chown("/tmp/chmod_test", myuid, mygid);
    if (ret != 0) {
        printf("CONTRACT_FAIL chown: errno=%d\n", errno);
        return 1;
    }
    stat("/tmp/chmod_test", &st);
    if (st.st_uid != myuid || st.st_gid != mygid) {
        printf("CONTRACT_FAIL chown_verify: uid=%d gid=%d\n", st.st_uid, st.st_gid);
        return 1;
    }
    printf("chown: ok\n");

    unlink("/tmp/chmod_test");
    printf("CONTRACT_PASS\n");
    return 0;
}
