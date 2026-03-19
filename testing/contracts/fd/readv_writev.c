/* Contract: writev scatter-gather writes; readv scatter-gather reads;
 * data concatenated correctly. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/uio.h>
#include <unistd.h>

int main(void) {
    int fd = open("/tmp/iov_test", O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (fd < 0) {
        printf("CONTRACT_FAIL open: errno=%d\n", errno);
        return 1;
    }

    /* writev with 3 iovecs */
    struct iovec wv[3];
    wv[0].iov_base = "Hello";
    wv[0].iov_len = 5;
    wv[1].iov_base = ", ";
    wv[1].iov_len = 2;
    wv[2].iov_base = "World!";
    wv[2].iov_len = 6;

    ssize_t nw = writev(fd, wv, 3);
    if (nw != 13) {
        printf("CONTRACT_FAIL writev: nw=%ld\n", (long)nw);
        return 1;
    }
    printf("writev: ok nw=%ld\n", (long)nw);

    /* readv back with 2 iovecs */
    lseek(fd, 0, SEEK_SET);
    char buf1[7] = {0};
    char buf2[6] = {0};
    struct iovec rv[2];
    rv[0].iov_base = buf1;
    rv[0].iov_len = 7;
    rv[1].iov_base = buf2;
    rv[1].iov_len = 6;

    ssize_t nr = readv(fd, rv, 2);
    if (nr != 13) {
        printf("CONTRACT_FAIL readv: nr=%ld\n", (long)nr);
        return 1;
    }
    if (memcmp(buf1, "Hello, ", 7) != 0) {
        printf("CONTRACT_FAIL readv_buf1: got '%s'\n", buf1);
        return 1;
    }
    if (memcmp(buf2, "World!", 6) != 0) {
        printf("CONTRACT_FAIL readv_buf2: got '%s'\n", buf2);
        return 1;
    }
    printf("readv: ok\n");

    close(fd);
    unlink("/tmp/iov_test");
    printf("CONTRACT_PASS\n");
    return 0;
}
