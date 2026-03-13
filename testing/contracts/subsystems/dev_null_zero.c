/* Contract: /dev/null absorbs all writes; /dev/zero reads as zeros. */
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int main(void) {
    /* /dev/null: write should succeed and return byte count */
    int fd = open("/dev/null", O_WRONLY);
    if (fd < 0) {
        printf("CONTRACT_FAIL open_null\n");
        return 1;
    }
    char data[] = "hello";
    int nw = write(fd, data, 5);
    if (nw != 5) {
        printf("CONTRACT_FAIL null_write expected=5 got=%d\n", nw);
        close(fd);
        return 1;
    }
    close(fd);
    printf("dev_null_write: ok\n");

    /* /dev/null: read should return 0 (EOF) */
    fd = open("/dev/null", O_RDONLY);
    if (fd < 0) {
        printf("CONTRACT_FAIL open_null_rd\n");
        return 1;
    }
    char buf[16];
    int nr = read(fd, buf, sizeof(buf));
    if (nr != 0) {
        printf("CONTRACT_FAIL null_read expected=0 got=%d\n", nr);
        close(fd);
        return 1;
    }
    close(fd);
    printf("dev_null_read: ok (eof)\n");

    /* /dev/zero: read should return all zeros */
    fd = open("/dev/zero", O_RDONLY);
    if (fd < 0) {
        printf("CONTRACT_FAIL open_zero\n");
        return 1;
    }
    memset(buf, 0xFF, sizeof(buf));
    nr = read(fd, buf, sizeof(buf));
    if (nr != (int)sizeof(buf)) {
        printf("CONTRACT_FAIL zero_read expected=%d got=%d\n", (int)sizeof(buf), nr);
        close(fd);
        return 1;
    }
    for (int i = 0; i < nr; i++) {
        if (buf[i] != 0) {
            printf("CONTRACT_FAIL zero_content byte[%d]=%d\n", i, (unsigned char)buf[i]);
            close(fd);
            return 1;
        }
    }
    close(fd);
    printf("dev_zero_read: ok (all zeros)\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
