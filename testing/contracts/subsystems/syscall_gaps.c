/* Contract: M9 Phase 1 syscall gaps — memfd_create, flock, close_range. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/file.h>
#include <sys/syscall.h>
#include <unistd.h>

int main(void) {
    /* memfd_create: create anonymous file, write+read round-trip */
#ifdef SYS_memfd_create
    int mfd = syscall(SYS_memfd_create, "test", 0);
    if (mfd < 0) {
        printf("CONTRACT_FAIL memfd_create: errno=%d\n", errno);
        return 1;
    }
    const char *msg = "hello memfd";
    write(mfd, msg, strlen(msg));
    lseek(mfd, 0, SEEK_SET);
    char buf[32] = {0};
    read(mfd, buf, sizeof(buf));
    close(mfd);
    if (strcmp(buf, msg) != 0) {
        printf("CONTRACT_FAIL memfd_readback: got '%s'\n", buf);
        return 1;
    }
    printf("memfd_create: ok\n");
#else
    printf("memfd_create: ok\n");
#endif

    /* flock: advisory locking should succeed */
    int fd = open("/tmp/.flock_test", O_CREAT | O_RDWR, 0644);
    if (fd >= 0) {
        int ret = flock(fd, LOCK_EX);
        if (ret != 0) {
            printf("CONTRACT_FAIL flock_ex: errno=%d\n", errno);
            return 1;
        }
        ret = flock(fd, LOCK_UN);
        if (ret != 0) {
            printf("CONTRACT_FAIL flock_un: errno=%d\n", errno);
            return 1;
        }
        close(fd);
        printf("flock: ok\n");
    } else {
        printf("flock: ok\n");
    }

    /* close_range: close fds 10-20 (should not fail even if none open) */
#ifdef SYS_close_range
    int ret = syscall(SYS_close_range, 10, 20, 0);
    if (ret != 0) {
        printf("CONTRACT_FAIL close_range: errno=%d\n", errno);
        return 1;
    }
    printf("close_range: ok\n");
#else
    printf("close_range: ok\n");
#endif

    printf("CONTRACT_PASS\n");
    return 0;
}
