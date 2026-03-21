/* Contract: fchmodat changes permissions via AT_FDCWD; fchownat changes
 * ownership via AT_FDCWD; stat reflects both. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <sys/stat.h>
#include <unistd.h>

int main(void) {
    /* Create test file */
    int fd = open("/tmp/fchmodat_test", O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (fd < 0) {
        printf("CONTRACT_FAIL open: errno=%d\n", errno);
        return 1;
    }
    close(fd);

    /* fchmodat to 0700 */
    int ret = fchmodat(AT_FDCWD, "/tmp/fchmodat_test", 0700, 0);
    if (ret != 0) {
        printf("CONTRACT_FAIL fchmodat: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    struct stat st;
    stat("/tmp/fchmodat_test", &st);
    if ((st.st_mode & 0777) != 0700) {
        printf("CONTRACT_FAIL fchmodat_verify: mode=0%03o\n", st.st_mode & 0777);
        return 1;
    }
    printf("fchmodat: ok\n");

    /* fchownat to own uid/gid */
    uid_t myuid = getuid();
    gid_t mygid = getgid();
    ret = fchownat(AT_FDCWD, "/tmp/fchmodat_test", myuid, mygid, 0);
    if (ret != 0) {
        printf("CONTRACT_FAIL fchownat: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    stat("/tmp/fchmodat_test", &st);
    if (st.st_uid != myuid || st.st_gid != mygid) {
        printf("CONTRACT_FAIL fchownat_verify\n");
        return 1;
    }
    printf("fchownat: ok\n");

    unlink("/tmp/fchmodat_test");
    printf("CONTRACT_PASS\n");
    return 0;
}
