/* Contract: utimensat accepts valid paths and rejects missing ones (stub). */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <sys/stat.h>
#include <unistd.h>

int main(void) {
    const char *path = "/tmp/utimens_test";
    int fd = open(path, O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (fd < 0) {
        printf("CONTRACT_FAIL create: errno=%d\n", errno);
        return 1;
    }
    close(fd);

    /* NULL times (set to current time) on valid path → 0 */
    if (utimensat(AT_FDCWD, path, NULL, 0) != 0) {
        printf("CONTRACT_FAIL utimensat_null: errno=%d\n", errno);
        return 1;
    }
    printf("utimensat_null: ok\n");

    /* UTIME_NOW on valid path → 0 */
    struct timespec times[2];
    times[0].tv_nsec = UTIME_NOW;
    times[1].tv_nsec = UTIME_NOW;
    if (utimensat(AT_FDCWD, path, times, 0) != 0) {
        printf("CONTRACT_FAIL utimensat_now: errno=%d\n", errno);
        return 1;
    }
    printf("utimensat_now: ok\n");

    /* Non-existent path → ENOENT */
    errno = 0;
    if (utimensat(AT_FDCWD, "/tmp/no_such_utimens_file", NULL, 0) != -1 ||
        errno != ENOENT) {
        printf("CONTRACT_FAIL utimensat_enoent: errno=%d\n", errno);
        return 1;
    }
    printf("utimensat_enoent: ok\n");

    unlink(path);
    printf("CONTRACT_PASS\n");
    return 0;
}
