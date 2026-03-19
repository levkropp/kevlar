/* Contract: ftruncate shrinks and extends files; extension zero-fills. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

int main(void) {
    int fd = open("/tmp/ftrunc_test", O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (fd < 0) {
        printf("CONTRACT_FAIL open: errno=%d\n", errno);
        return 1;
    }

    /* Write "hello world" */
    if (write(fd, "hello world", 11) != 11) {
        printf("CONTRACT_FAIL write: errno=%d\n", errno);
        return 1;
    }

    /* Shrink to 5 bytes */
    if (ftruncate(fd, 5) != 0) {
        printf("CONTRACT_FAIL ftruncate_shrink: errno=%d\n", errno);
        return 1;
    }
    struct stat st;
    fstat(fd, &st);
    if (st.st_size != 5) {
        printf("CONTRACT_FAIL shrink_size: size=%ld\n", (long)st.st_size);
        return 1;
    }
    lseek(fd, 0, SEEK_SET);
    char buf[32] = {0};
    if (read(fd, buf, 5) != 5 || memcmp(buf, "hello", 5) != 0) {
        printf("CONTRACT_FAIL shrink_data: buf='%s'\n", buf);
        return 1;
    }
    printf("shrink: ok size=5\n");

    /* Extend to 20 bytes — bytes 5..19 must be zero */
    if (ftruncate(fd, 20) != 0) {
        printf("CONTRACT_FAIL ftruncate_extend: errno=%d\n", errno);
        return 1;
    }
    fstat(fd, &st);
    if (st.st_size != 20) {
        printf("CONTRACT_FAIL extend_size: size=%ld\n", (long)st.st_size);
        return 1;
    }
    lseek(fd, 0, SEEK_SET);
    memset(buf, 0xFF, sizeof(buf));
    if (read(fd, buf, 20) != 20) {
        printf("CONTRACT_FAIL extend_read: errno=%d\n", errno);
        return 1;
    }
    if (memcmp(buf, "hello", 5) != 0) {
        printf("CONTRACT_FAIL extend_prefix: corrupted\n");
        return 1;
    }
    int zeros_ok = 1;
    for (int i = 5; i < 20; i++) {
        if (buf[i] != 0) {
            zeros_ok = 0;
            break;
        }
    }
    if (!zeros_ok) {
        printf("CONTRACT_FAIL extend_zeros: non-zero in extended region\n");
        return 1;
    }
    printf("extend: ok size=20\n");

    close(fd);
    unlink("/tmp/ftrunc_test");
    printf("CONTRACT_PASS\n");
    return 0;
}
