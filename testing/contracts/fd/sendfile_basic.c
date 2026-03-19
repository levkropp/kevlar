/* Contract: sendfile zero-copy file-to-pipe transfer;
 * NULL offset updates file position; non-NULL offset independent. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/sendfile.h>
#include <unistd.h>

int main(void) {
    /* Create source file */
    int src = open("/tmp/sf_src", O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (src < 0) {
        printf("CONTRACT_FAIL open_src: errno=%d\n", errno);
        return 1;
    }
    write(src, "ABCDEFGHIJ", 10);
    lseek(src, 0, SEEK_SET);

    int fds[2];
    pipe(fds);

    /* sendfile with NULL offset: uses and updates file position */
    ssize_t sent = sendfile(fds[1], src, NULL, 5);
    if (sent != 5) {
        printf("CONTRACT_FAIL sendfile_null: sent=%ld errno=%d\n", (long)sent, errno);
        return 1;
    }
    off_t pos = lseek(src, 0, SEEK_CUR);
    if (pos != 5) {
        printf("CONTRACT_FAIL null_offset_pos: pos=%ld expected=5\n", (long)pos);
        return 1;
    }
    char buf[16] = {0};
    read(fds[0], buf, 5);
    if (memcmp(buf, "ABCDE", 5) != 0) {
        printf("CONTRACT_FAIL null_data: got '%s'\n", buf);
        return 1;
    }
    printf("sendfile_null: ok\n");

    /* sendfile with non-NULL offset: does NOT change file position */
    off_t off = 2;
    sent = sendfile(fds[1], src, &off, 4);
    if (sent != 4) {
        printf("CONTRACT_FAIL sendfile_off: sent=%ld errno=%d\n", (long)sent, errno);
        return 1;
    }
    /* offset variable updated */
    if (off != 6) {
        printf("CONTRACT_FAIL off_updated: off=%ld expected=6\n", (long)off);
        return 1;
    }
    /* file position unchanged */
    pos = lseek(src, 0, SEEK_CUR);
    if (pos != 5) {
        printf("CONTRACT_FAIL off_pos: pos=%ld expected=5\n", (long)pos);
        return 1;
    }
    memset(buf, 0, sizeof(buf));
    read(fds[0], buf, 4);
    if (memcmp(buf, "CDEF", 4) != 0) {
        printf("CONTRACT_FAIL off_data: got '%s'\n", buf);
        return 1;
    }
    printf("sendfile_offset: ok\n");

    close(fds[0]);
    close(fds[1]);
    close(src);
    unlink("/tmp/sf_src");
    printf("CONTRACT_PASS\n");
    return 0;
}
