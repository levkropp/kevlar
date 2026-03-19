/* Contract: preadv/pwritev do not advance file offset. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/uio.h>
#include <unistd.h>

int main(void) {
    int fd = open("/tmp/preadv_test", O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (fd < 0) {
        printf("CONTRACT_FAIL open: errno=%d\n", errno);
        return 1;
    }

    /* Write 10 bytes to establish file size and position */
    if (write(fd, "0123456789", 10) != 10) {
        printf("CONTRACT_FAIL write: errno=%d\n", errno);
        return 1;
    }
    off_t pos = lseek(fd, 0, SEEK_CUR);
    if (pos != 10) {
        printf("CONTRACT_FAIL initial_pos: pos=%ld\n", (long)pos);
        return 1;
    }
    printf("setup: ok pos=%ld\n", (long)pos);

    /* pwritev two iovecs at offset 0; file offset must remain 10 */
    struct iovec wv[2];
    wv[0].iov_base = "ABCD";
    wv[0].iov_len = 4;
    wv[1].iov_base = "EF";
    wv[1].iov_len = 2;

    ssize_t nw = pwritev(fd, wv, 2, 0);
    if (nw != 6) {
        printf("CONTRACT_FAIL pwritev: nw=%ld errno=%d\n", (long)nw, errno);
        return 1;
    }
    pos = lseek(fd, 0, SEEK_CUR);
    if (pos != 10) {
        printf("CONTRACT_FAIL pwritev_pos: pos=%ld expected=10\n", (long)pos);
        return 1;
    }
    printf("pwritev: ok pos=%ld\n", (long)pos);

    /* preadv two iovecs from offset 2; file offset must remain 10 */
    char buf1[3] = {0};
    char buf2[3] = {0};
    struct iovec rv[2];
    rv[0].iov_base = buf1;
    rv[0].iov_len = 3;
    rv[1].iov_base = buf2;
    rv[1].iov_len = 3;

    ssize_t nr = preadv(fd, rv, 2, 2);
    if (nr != 6) {
        printf("CONTRACT_FAIL preadv: nr=%ld errno=%d\n", (long)nr, errno);
        return 1;
    }
    /* offset 2: "CDEF67" (overwritten first 6 bytes = "ABCDEF", rest = "6789") */
    if (memcmp(buf1, "CDE", 3) != 0) {
        printf("CONTRACT_FAIL preadv_buf1: got '%s'\n", buf1);
        return 1;
    }
    if (memcmp(buf2, "F67", 3) != 0) {
        printf("CONTRACT_FAIL preadv_buf2: got '%s'\n", buf2);
        return 1;
    }
    pos = lseek(fd, 0, SEEK_CUR);
    if (pos != 10) {
        printf("CONTRACT_FAIL preadv_pos: pos=%ld expected=10\n", (long)pos);
        return 1;
    }
    printf("preadv: ok pos=%ld\n", (long)pos);

    close(fd);
    unlink("/tmp/preadv_test");
    printf("CONTRACT_PASS\n");
    return 0;
}
