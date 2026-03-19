/* Contract: pipe read/write, EOF on close, pipe2 O_NONBLOCK EAGAIN. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int main(void) {
    int fds[2];

    /* Basic pipe */
    if (pipe(fds) != 0) {
        printf("CONTRACT_FAIL pipe: errno=%d\n", errno);
        return 1;
    }

    /* Write-read roundtrip */
    write(fds[1], "test", 4);
    char buf[16] = {0};
    int n = read(fds[0], buf, sizeof(buf));
    if (n != 4 || memcmp(buf, "test", 4) != 0) {
        printf("CONTRACT_FAIL roundtrip: n=%d buf=%s\n", n, buf);
        return 1;
    }
    printf("roundtrip: ok\n");

    /* Close write end → read returns 0 (EOF) */
    close(fds[1]);
    n = read(fds[0], buf, sizeof(buf));
    if (n != 0) {
        printf("CONTRACT_FAIL eof: n=%d\n", n);
        return 1;
    }
    printf("eof: ok\n");
    close(fds[0]);

    /* pipe2 with O_NONBLOCK */
    if (pipe2(fds, O_NONBLOCK) != 0) {
        printf("CONTRACT_FAIL pipe2: errno=%d\n", errno);
        return 1;
    }
    errno = 0;
    n = read(fds[0], buf, sizeof(buf));
    if (n != -1 || errno != EAGAIN) {
        printf("CONTRACT_FAIL nonblock: n=%d errno=%d\n", n, errno);
        return 1;
    }
    printf("nonblock_eagain: ok\n");

    /* pipe2 O_CLOEXEC */
    close(fds[0]);
    close(fds[1]);
    if (pipe2(fds, O_CLOEXEC) != 0) {
        printf("CONTRACT_FAIL pipe2_cloexec: errno=%d\n", errno);
        return 1;
    }
    int flags = fcntl(fds[0], F_GETFD);
    if (!(flags & FD_CLOEXEC)) {
        printf("CONTRACT_FAIL cloexec: flags=0x%x\n", flags);
        return 1;
    }
    printf("pipe2_cloexec: ok\n");

    close(fds[0]);
    close(fds[1]);
    printf("CONTRACT_PASS\n");
    return 0;
}
