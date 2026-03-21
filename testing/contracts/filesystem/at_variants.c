/* Contract: *at syscall variants (linkat, unlinkat, symlinkat, readlinkat,
 * mkdirat, renameat) work with AT_FDCWD. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

int main(void) {
    /* mkdirat with AT_FDCWD */
    int ret = mkdirat(AT_FDCWD, "/tmp/at_test_dir", 0755);
    if (ret != 0) {
        printf("CONTRACT_FAIL mkdirat: errno=%d\n", errno);
        return 1;
    }
    struct stat st;
    if (stat("/tmp/at_test_dir", &st) != 0 || !S_ISDIR(st.st_mode)) {
        printf("CONTRACT_FAIL mkdirat_verify: errno=%d\n", errno);
        return 1;
    }
    printf("mkdirat: ok\n");

    /* Create a file to link/rename */
    int fd = openat(AT_FDCWD, "/tmp/at_test_file", O_CREAT | O_WRONLY, 0644);
    if (fd < 0) {
        printf("CONTRACT_FAIL openat_create: errno=%d\n", errno);
        return 1;
    }
    write(fd, "test", 4);
    close(fd);

    /* linkat with AT_FDCWD */
    ret = linkat(AT_FDCWD, "/tmp/at_test_file", AT_FDCWD, "/tmp/at_test_link", 0);
    if (ret != 0) {
        printf("CONTRACT_FAIL linkat: errno=%d\n", errno);
        return 1;
    }
    if (stat("/tmp/at_test_link", &st) != 0) {
        printf("CONTRACT_FAIL linkat_verify\n");
        return 1;
    }
    printf("linkat: ok\n");

    /* symlinkat */
    ret = symlinkat("/tmp/at_test_file", AT_FDCWD, "/tmp/at_test_sym");
    if (ret != 0) {
        printf("CONTRACT_FAIL symlinkat: errno=%d\n", errno);
        return 1;
    }

    /* readlinkat */
    char buf[256] = {0};
    ssize_t n = readlinkat(AT_FDCWD, "/tmp/at_test_sym", buf, sizeof(buf) - 1);
    if (n < 0 || strcmp(buf, "/tmp/at_test_file") != 0) {
        printf("CONTRACT_FAIL readlinkat: n=%ld buf=%s\n", (long)n, buf);
        return 1;
    }
    printf("symlinkat_readlinkat: ok\n");

    /* renameat */
    ret = renameat(AT_FDCWD, "/tmp/at_test_file", AT_FDCWD, "/tmp/at_test_renamed");
    if (ret != 0) {
        printf("CONTRACT_FAIL renameat: errno=%d\n", errno);
        return 1;
    }
    if (stat("/tmp/at_test_renamed", &st) != 0) {
        printf("CONTRACT_FAIL renameat_verify\n");
        return 1;
    }
    printf("renameat: ok\n");

    /* unlinkat (file) */
    ret = unlinkat(AT_FDCWD, "/tmp/at_test_renamed", 0);
    if (ret != 0) {
        printf("CONTRACT_FAIL unlinkat_file: errno=%d\n", errno);
        return 1;
    }
    printf("unlinkat_file: ok\n");

    /* unlinkat (directory, AT_REMOVEDIR) */
    ret = unlinkat(AT_FDCWD, "/tmp/at_test_dir", AT_REMOVEDIR);
    if (ret != 0) {
        printf("CONTRACT_FAIL unlinkat_dir: errno=%d\n", errno);
        return 1;
    }
    printf("unlinkat_dir: ok\n");

    /* Cleanup */
    unlink("/tmp/at_test_link");
    unlink("/tmp/at_test_sym");

    printf("CONTRACT_PASS\n");
    return 0;
}
