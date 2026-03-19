/* Contract: unlink removes file; rename replaces target atomically;
 * rename to self succeeds. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

int main(void) {
    const char *f1 = "/tmp/contract_f1";
    const char *f2 = "/tmp/contract_f2";

    /* Create file */
    int fd = open(f1, O_CREAT | O_WRONLY | O_TRUNC, 0644);
    write(fd, "aaa", 3);
    close(fd);

    /* Unlink removes it */
    if (unlink(f1) != 0) {
        printf("CONTRACT_FAIL unlink: errno=%d\n", errno);
        return 1;
    }
    struct stat st;
    if (stat(f1, &st) != -1 || errno != ENOENT) {
        printf("CONTRACT_FAIL unlink_gone: errno=%d\n", errno);
        return 1;
    }
    printf("unlink: ok\n");

    /* Create two files for rename */
    fd = open(f1, O_CREAT | O_WRONLY | O_TRUNC, 0644);
    write(fd, "src", 3);
    close(fd);
    fd = open(f2, O_CREAT | O_WRONLY | O_TRUNC, 0644);
    write(fd, "dst", 3);
    close(fd);

    /* Rename replaces target */
    if (rename(f1, f2) != 0) {
        printf("CONTRACT_FAIL rename: errno=%d\n", errno);
        return 1;
    }
    /* f1 gone */
    if (stat(f1, &st) != -1) {
        printf("CONTRACT_FAIL rename_src_gone\n");
        return 1;
    }
    /* f2 has src content */
    char buf[8] = {0};
    fd = open(f2, O_RDONLY);
    read(fd, buf, 3);
    close(fd);
    if (memcmp(buf, "src", 3) != 0) {
        printf("CONTRACT_FAIL rename_content: buf=%s\n", buf);
        return 1;
    }
    printf("rename_replaces: ok\n");

    /* Rename to self succeeds */
    if (rename(f2, f2) != 0) {
        printf("CONTRACT_FAIL rename_self: errno=%d\n", errno);
        return 1;
    }
    printf("rename_self: ok\n");

    unlink(f2);
    printf("CONTRACT_PASS\n");
    return 0;
}
