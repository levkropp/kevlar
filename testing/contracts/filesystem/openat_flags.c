/* Contract: openat O_CREAT|O_EXCL, O_TRUNC, O_APPEND semantics. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

int main(void) {
    const char *path = "/tmp/openat_flags_test";
    unlink(path);

    /* O_CREAT|O_EXCL: first succeeds, second fails EEXIST */
    int fd = openat(AT_FDCWD, path, O_CREAT | O_EXCL | O_RDWR, 0644);
    if (fd < 0) {
        printf("CONTRACT_FAIL creat_excl: errno=%d\n", errno);
        return 1;
    }
    write(fd, "data", 4);
    close(fd);

    fd = openat(AT_FDCWD, path, O_CREAT | O_EXCL | O_RDWR, 0644);
    if (fd != -1 || errno != EEXIST) {
        printf("CONTRACT_FAIL excl_dup: fd=%d errno=%d\n", fd, errno);
        if (fd >= 0) close(fd);
        return 1;
    }
    printf("o_excl: ok\n");

    /* O_CREAT on existing without O_EXCL: no error */
    fd = openat(AT_FDCWD, path, O_CREAT | O_RDWR, 0644);
    if (fd < 0) {
        printf("CONTRACT_FAIL creat_noexcl: errno=%d\n", errno);
        return 1;
    }
    close(fd);
    printf("o_creat_existing: ok\n");

    /* O_TRUNC: size becomes 0 */
    fd = openat(AT_FDCWD, path, O_RDWR | O_TRUNC, 0);
    if (fd < 0) {
        printf("CONTRACT_FAIL o_trunc: errno=%d\n", errno);
        return 1;
    }
    struct stat st;
    fstat(fd, &st);
    if (st.st_size != 0) {
        printf("CONTRACT_FAIL trunc_size: size=%ld\n", (long)st.st_size);
        close(fd);
        return 1;
    }
    close(fd);
    printf("o_trunc: ok\n");

    /* O_APPEND: two writers → concatenation */
    unlink(path);
    int fd1 = openat(AT_FDCWD, path, O_CREAT | O_WRONLY | O_APPEND, 0644);
    int fd2 = openat(AT_FDCWD, path, O_WRONLY | O_APPEND, 0);
    if (fd1 < 0 || fd2 < 0) {
        printf("CONTRACT_FAIL append_open: fd1=%d fd2=%d errno=%d\n", fd1, fd2, errno);
        return 1;
    }
    write(fd1, "AAA", 3);
    write(fd2, "BBB", 3);
    write(fd1, "CCC", 3);
    close(fd1);
    close(fd2);

    fd = openat(AT_FDCWD, path, O_RDONLY, 0);
    char buf[16] = {0};
    ssize_t n = read(fd, buf, sizeof(buf));
    close(fd);
    if (n != 9 || memcmp(buf, "AAABBBCCC", 9) != 0) {
        printf("CONTRACT_FAIL append_data: n=%ld buf='%s'\n", (long)n, buf);
        return 1;
    }
    printf("o_append: ok\n");

    unlink(path);
    printf("CONTRACT_PASS\n");
    return 0;
}
