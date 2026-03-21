/* Contract: copy_file_range transfers data between fds; offsets advance;
 * zero-length copy returns 0; EBADF on bad fd. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>

static ssize_t my_copy_file_range(int fd_in, long long *off_in,
                                   int fd_out, long long *off_out,
                                   size_t len, unsigned int flags) {
    return syscall(SYS_copy_file_range, fd_in, off_in, fd_out, off_out, len, flags);
}

int main(void) {
    /* Create source file with data */
    int src = open("/tmp/cfr_src", O_CREAT | O_RDWR | O_TRUNC, 0644);
    int dst = open("/tmp/cfr_dst", O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (src < 0 || dst < 0) {
        printf("CONTRACT_FAIL open: src=%d dst=%d errno=%d\n", src, dst, errno);
        return 1;
    }

    const char *data = "hello copy_file_range";
    write(src, data, strlen(data));
    lseek(src, 0, SEEK_SET);

    /* Copy all data */
    ssize_t n = my_copy_file_range(src, NULL, dst, NULL, strlen(data), 0);
    if (n != (ssize_t)strlen(data)) {
        printf("CONTRACT_FAIL copy: n=%ld errno=%d\n", (long)n, errno);
        close(src); close(dst);
        return 1;
    }
    printf("copy: ok\n");

    /* Verify data */
    lseek(dst, 0, SEEK_SET);
    char buf[64] = {0};
    read(dst, buf, sizeof(buf));
    if (memcmp(buf, data, strlen(data)) != 0) {
        printf("CONTRACT_FAIL verify: buf=%s\n", buf);
        close(src); close(dst);
        return 1;
    }
    printf("verify: ok\n");

    /* Copy with explicit offsets */
    long long off_in = 6, off_out = 0;
    int dst2 = open("/tmp/cfr_dst2", O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (dst2 < 0) {
        printf("CONTRACT_FAIL open_dst2: errno=%d\n", errno);
        close(src); close(dst);
        return 1;
    }
    n = my_copy_file_range(src, &off_in, dst2, &off_out, 15, 0);
    if (n != 15) {
        printf("CONTRACT_FAIL offset_copy: n=%ld errno=%d\n", (long)n, errno);
        close(src); close(dst); close(dst2);
        return 1;
    }
    printf("offset_copy: ok\n");

    /* Zero-length copy */
    n = my_copy_file_range(src, NULL, dst, NULL, 0, 0);
    if (n != 0) {
        printf("CONTRACT_FAIL zero_len: n=%ld errno=%d\n", (long)n, errno);
        close(src); close(dst); close(dst2);
        return 1;
    }
    printf("zero_len: ok\n");

    close(src); close(dst); close(dst2);
    unlink("/tmp/cfr_src");
    unlink("/tmp/cfr_dst");
    unlink("/tmp/cfr_dst2");

    printf("CONTRACT_PASS\n");
    return 0;
}
