/* Contract: fchmod/fchmodat accept calls without error (stub). */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <sys/stat.h>
#include <unistd.h>

int main(void) {
    const char *path = "/tmp/fchmod_test";
    int fd = open(path, O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (fd < 0) {
        printf("CONTRACT_FAIL open: errno=%d\n", errno);
        return 1;
    }

    /* fchmod to read-only → 0 (stub) */
    if (fchmod(fd, 0400) != 0) {
        printf("CONTRACT_FAIL fchmod: errno=%d\n", errno);
        close(fd);
        return 1;
    }
    printf("fchmod: ok\n");

    /* Write still works (no enforcement in stub) */
    ssize_t n = write(fd, "test", 4);
    if (n != 4) {
        printf("CONTRACT_FAIL write_after_chmod: n=%ld errno=%d\n", (long)n, errno);
        close(fd);
        return 1;
    }
    printf("write_after_chmod: ok\n");

    /* fchmodat on path → 0 (stub) */
    if (fchmodat(AT_FDCWD, path, 0644, 0) != 0) {
        printf("CONTRACT_FAIL fchmodat: errno=%d\n", errno);
        close(fd);
        return 1;
    }
    printf("fchmodat: ok\n");

    close(fd);
    unlink(path);
    printf("CONTRACT_PASS\n");
    return 0;
}
