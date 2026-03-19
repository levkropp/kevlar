/* Contract: lseek SEEK_SET/CUR/END, beyond-EOF seek, ESPIPE on pipe. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int main(void) {
    int fd = open("/tmp/lseek_test", O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (fd < 0) {
        printf("CONTRACT_FAIL open: errno=%d\n", errno);
        return 1;
    }
    write(fd, "abcdefghij", 10);

    /* SEEK_SET */
    off_t pos = lseek(fd, 3, SEEK_SET);
    if (pos != 3) {
        printf("CONTRACT_FAIL seek_set: pos=%ld\n", (long)pos);
        return 1;
    }
    char buf[4] = {0};
    read(fd, buf, 3);
    if (memcmp(buf, "def", 3) != 0) {
        printf("CONTRACT_FAIL seek_set_read: buf=%s\n", buf);
        return 1;
    }
    printf("seek_set: ok\n");

    /* SEEK_CUR: relative from current (now at 6) */
    pos = lseek(fd, -2, SEEK_CUR);
    if (pos != 4) {
        printf("CONTRACT_FAIL seek_cur: pos=%ld expected=4\n", (long)pos);
        return 1;
    }
    printf("seek_cur: ok pos=%ld\n", (long)pos);

    /* SEEK_END: relative from end (size=10) */
    pos = lseek(fd, -3, SEEK_END);
    if (pos != 7) {
        printf("CONTRACT_FAIL seek_end: pos=%ld expected=7\n", (long)pos);
        return 1;
    }
    read(fd, buf, 3);
    if (memcmp(buf, "hij", 3) != 0) {
        printf("CONTRACT_FAIL seek_end_read: buf=%s\n", buf);
        return 1;
    }
    printf("seek_end: ok\n");

    /* Seek beyond EOF */
    pos = lseek(fd, 100, SEEK_SET);
    if (pos != 100) {
        printf("CONTRACT_FAIL seek_beyond: pos=%ld\n", (long)pos);
        return 1;
    }
    printf("seek_beyond: ok pos=%ld\n", (long)pos);

    close(fd);
    unlink("/tmp/lseek_test");

    /* ESPIPE on pipe */
    int fds[2];
    pipe(fds);
    errno = 0;
    pos = lseek(fds[0], 0, SEEK_CUR);
    if (pos != (off_t)-1 || errno != ESPIPE) {
        printf("CONTRACT_FAIL espipe: pos=%ld errno=%d\n", (long)pos, errno);
        return 1;
    }
    printf("espipe: ok\n");
    close(fds[0]);
    close(fds[1]);

    printf("CONTRACT_PASS\n");
    return 0;
}
