/* Contract: fchown changes file owner/group; fstat reflects the change;
 * chown by path works; -1 means "don't change that field".
 * Note: chown to different uid requires root — test adapts to both. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <sys/stat.h>
#include <unistd.h>

int main(void) {
    /* Create a temp file */
    int fd = open("/tmp/chown_test", O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (fd < 0) {
        printf("CONTRACT_FAIL open: errno=%d\n", errno);
        return 1;
    }

    /* Get current uid/gid */
    uid_t myuid = getuid();
    gid_t mygid = getgid();

    /* fchown to own uid/gid (always permitted) */
    int ret = fchown(fd, myuid, mygid);
    if (ret != 0) {
        printf("CONTRACT_FAIL fchown_self: ret=%d errno=%d\n", ret, errno);
        close(fd);
        return 1;
    }

    struct stat st;
    fstat(fd, &st);
    if (st.st_uid != myuid || st.st_gid != mygid) {
        printf("CONTRACT_FAIL fchown_check: uid=%d gid=%d\n", st.st_uid, st.st_gid);
        close(fd);
        return 1;
    }
    printf("fchown_self: ok\n");

    /* fchown with -1 for uid: should not change uid */
    ret = fchown(fd, -1, mygid);
    if (ret != 0) {
        printf("CONTRACT_FAIL fchown_nop_uid: ret=%d errno=%d\n", ret, errno);
        close(fd);
        return 1;
    }
    fstat(fd, &st);
    if (st.st_uid != myuid) {
        printf("CONTRACT_FAIL nop_uid_check: uid=%d\n", st.st_uid);
        close(fd);
        return 1;
    }
    printf("fchown_nop_uid: ok\n");

    /* fchown with -1 for gid: should not change gid */
    ret = fchown(fd, myuid, -1);
    if (ret != 0) {
        printf("CONTRACT_FAIL fchown_nop_gid: ret=%d errno=%d\n", ret, errno);
        close(fd);
        return 1;
    }
    fstat(fd, &st);
    if (st.st_gid != mygid) {
        printf("CONTRACT_FAIL nop_gid_check: gid=%d\n", st.st_gid);
        close(fd);
        return 1;
    }
    printf("fchown_nop_gid: ok\n");

    close(fd);

    /* chown by path (to own uid/gid) */
    ret = chown("/tmp/chown_test", myuid, mygid);
    if (ret != 0) {
        printf("CONTRACT_FAIL chown_path: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("chown_path: ok\n");

    unlink("/tmp/chown_test");
    printf("CONTRACT_PASS\n");
    return 0;
}
