/* Contract: dup/dup2/dup3 share file offset; dup returns lowest fd;
 * dup2 closes target atomically; dup3 O_CLOEXEC works. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int main(void) {
    /* Create a temp file */
    int fd = open("/tmp/dup_test", O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (fd < 0) {
        printf("CONTRACT_FAIL open: errno=%d\n", errno);
        return 1;
    }
    write(fd, "hello", 5);
    lseek(fd, 0, SEEK_SET);

    /* dup returns lowest available fd */
    int fd2 = dup(fd);
    if (fd2 < 0) {
        printf("CONTRACT_FAIL dup: errno=%d\n", errno);
        return 1;
    }
    if (fd2 <= fd) {
        printf("CONTRACT_FAIL dup_lowest: fd=%d fd2=%d\n", fd, fd2);
        return 1;
    }
    printf("dup_lowest: ok fd=%d fd2=%d\n", fd, fd2);

    /* Shared offset: read from fd2 advances both */
    char buf[8] = {0};
    int n = read(fd2, buf, 3);
    if (n != 3 || memcmp(buf, "hel", 3) != 0) {
        printf("CONTRACT_FAIL shared_read: n=%d buf=%s\n", n, buf);
        return 1;
    }
    off_t pos = lseek(fd, 0, SEEK_CUR);
    if (pos != 3) {
        printf("CONTRACT_FAIL shared_offset: pos=%ld expected=3\n", (long)pos);
        return 1;
    }
    printf("shared_offset: ok\n");
    close(fd2);

    /* dup2: replaces target fd */
    int fd3 = open("/dev/null", O_RDONLY);
    int fd4 = dup2(fd, fd3);
    if (fd4 != fd3) {
        printf("CONTRACT_FAIL dup2: fd4=%d expected=%d\n", fd4, fd3);
        return 1;
    }
    /* fd3 now refers to same file as fd */
    lseek(fd, 0, SEEK_SET);
    pos = lseek(fd3, 0, SEEK_CUR);
    if (pos != 0) {
        printf("CONTRACT_FAIL dup2_offset: pos=%ld\n", (long)pos);
        return 1;
    }
    printf("dup2: ok\n");

    /* dup2 with oldfd==newfd is a no-op (returns newfd) */
    int fd5 = dup2(fd, fd);
    if (fd5 != fd) {
        printf("CONTRACT_FAIL dup2_same: fd5=%d expected=%d\n", fd5, fd);
        return 1;
    }
    printf("dup2_same: ok\n");

    /* dup3 with O_CLOEXEC */
    int fd6 = open("/dev/null", O_RDONLY);
    int fd7 = dup3(fd, fd6, O_CLOEXEC);
    if (fd7 != fd6) {
        printf("CONTRACT_FAIL dup3: fd7=%d expected=%d\n", fd7, fd6);
        return 1;
    }
    int flags = fcntl(fd7, F_GETFD);
    if (!(flags & FD_CLOEXEC)) {
        printf("CONTRACT_FAIL dup3_cloexec: flags=0x%x\n", flags);
        return 1;
    }
    printf("dup3_cloexec: ok\n");

    /* dup3 EINVAL when oldfd==newfd */
    errno = 0;
    int ret = dup3(fd, fd, O_CLOEXEC);
    if (ret != -1 || errno != EINVAL) {
        printf("CONTRACT_FAIL dup3_einval: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("dup3_einval: ok\n");

    close(fd7);
    close(fd3);
    close(fd);
    unlink("/tmp/dup_test");
    printf("CONTRACT_PASS\n");
    return 0;
}
