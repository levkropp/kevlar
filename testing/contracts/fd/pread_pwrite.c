/* Contract: pread64/pwrite64 do not advance file offset. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int main(void) {
    int fd = open("/tmp/pread_test", O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (fd < 0) {
        printf("CONTRACT_FAIL open: errno=%d\n", errno);
        return 1;
    }

    /* Write "hello world" (11 bytes), offset now at 11 */
    if (write(fd, "hello world", 11) != 11) {
        printf("CONTRACT_FAIL write: errno=%d\n", errno);
        return 1;
    }
    off_t pos = lseek(fd, 0, SEEK_CUR);
    if (pos != 11) {
        printf("CONTRACT_FAIL initial_pos: pos=%ld\n", (long)pos);
        return 1;
    }
    printf("setup: ok pos=%ld\n", (long)pos);

    /* pread at offset 6 → "world"; file offset must remain 11 */
    char buf[16] = {0};
    ssize_t n = pread(fd, buf, 5, 6);
    if (n != 5 || memcmp(buf, "world", 5) != 0) {
        printf("CONTRACT_FAIL pread: n=%ld buf='%s'\n", (long)n, buf);
        return 1;
    }
    pos = lseek(fd, 0, SEEK_CUR);
    if (pos != 11) {
        printf("CONTRACT_FAIL pread_pos: pos=%ld expected=11\n", (long)pos);
        return 1;
    }
    printf("pread: ok pos=%ld\n", (long)pos);

    /* pwrite "HELLO" at offset 0; file offset must remain 11 */
    n = pwrite(fd, "HELLO", 5, 0);
    if (n != 5) {
        printf("CONTRACT_FAIL pwrite: n=%ld errno=%d\n", (long)n, errno);
        return 1;
    }
    pos = lseek(fd, 0, SEEK_CUR);
    if (pos != 11) {
        printf("CONTRACT_FAIL pwrite_pos: pos=%ld expected=11\n", (long)pos);
        return 1;
    }
    printf("pwrite: ok pos=%ld\n", (long)pos);

    /* Verify file contents: "HELLO world" */
    lseek(fd, 0, SEEK_SET);
    memset(buf, 0, sizeof(buf));
    if (read(fd, buf, 11) != 11 || memcmp(buf, "HELLO world", 11) != 0) {
        printf("CONTRACT_FAIL verify: buf='%s'\n", buf);
        return 1;
    }
    printf("verify: ok\n");

    close(fd);
    unlink("/tmp/pread_test");
    printf("CONTRACT_PASS\n");
    return 0;
}
