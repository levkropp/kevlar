/* Contract: splice transfers data between pipe and file. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int main(void) {
    int fd = open("/tmp/splice_test", O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (fd < 0) {
        printf("CONTRACT_FAIL open: errno=%d\n", errno);
        return 1;
    }
    if (write(fd, "0123456789", 10) != 10) {
        printf("CONTRACT_FAIL write: errno=%d\n", errno);
        return 1;
    }
    lseek(fd, 0, SEEK_SET);

    int pfd[2];
    if (pipe(pfd) != 0) {
        printf("CONTRACT_FAIL pipe: errno=%d\n", errno);
        return 1;
    }

    /* splice entire file → pipe */
    ssize_t n = splice(fd, NULL, pfd[1], NULL, 10, 0);
    if (n != 10) {
        printf("CONTRACT_FAIL splice_all: n=%ld errno=%d\n", (long)n, errno);
        return 1;
    }
    char buf[16] = {0};
    if (read(pfd[0], buf, 16) != 10 || memcmp(buf, "0123456789", 10) != 0) {
        printf("CONTRACT_FAIL splice_all_data: buf='%s'\n", buf);
        return 1;
    }
    printf("splice_all: ok\n");

    /* splice with explicit offset → last 5 bytes */
    off_t off_in = 5;
    n = splice(fd, &off_in, pfd[1], NULL, 5, 0);
    if (n != 5) {
        printf("CONTRACT_FAIL splice_offset: n=%ld errno=%d\n", (long)n, errno);
        return 1;
    }
    memset(buf, 0, sizeof(buf));
    if (read(pfd[0], buf, 16) != 5 || memcmp(buf, "56789", 5) != 0) {
        printf("CONTRACT_FAIL splice_offset_data: buf='%s'\n", buf);
        return 1;
    }
    printf("splice_offset: ok\n");

    close(pfd[0]);
    close(pfd[1]);
    close(fd);
    unlink("/tmp/splice_test");
    printf("CONTRACT_PASS\n");
    return 0;
}
