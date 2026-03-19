/* Contract: symlink creation; readlink returns target;
 * open follows; lstat shows S_IFLNK; stat follows. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

int main(void) {
    const char *target = "/tmp/contract_sym_target";
    const char *link = "/tmp/contract_sym_link";
    unlink(link);
    unlink(target);

    /* Create target file */
    int fd = open(target, O_CREAT | O_WRONLY | O_TRUNC, 0644);
    write(fd, "data", 4);
    close(fd);

    /* Create symlink */
    if (symlink(target, link) != 0) {
        printf("CONTRACT_FAIL symlink: errno=%d\n", errno);
        return 1;
    }
    printf("symlink: ok\n");

    /* readlink returns target path */
    char buf[256] = {0};
    ssize_t n = readlink(link, buf, sizeof(buf) - 1);
    if (n < 0 || strcmp(buf, target) != 0) {
        printf("CONTRACT_FAIL readlink: n=%ld buf=%s\n", (long)n, buf);
        return 1;
    }
    printf("readlink: ok\n");

    /* open follows symlink → reads target data */
    fd = open(link, O_RDONLY);
    if (fd < 0) {
        printf("CONTRACT_FAIL open_follow: errno=%d\n", errno);
        return 1;
    }
    char data[8] = {0};
    read(fd, data, 4);
    close(fd);
    if (memcmp(data, "data", 4) != 0) {
        printf("CONTRACT_FAIL open_content: data=%s\n", data);
        return 1;
    }
    printf("open_follow: ok\n");

    /* lstat shows S_IFLNK */
    struct stat lst;
    if (lstat(link, &lst) != 0) {
        printf("CONTRACT_FAIL lstat: errno=%d\n", errno);
        return 1;
    }
    if (!S_ISLNK(lst.st_mode)) {
        printf("CONTRACT_FAIL lstat_islnk: mode=0x%x\n", lst.st_mode);
        return 1;
    }
    printf("lstat_islnk: ok\n");

    /* stat follows → S_ISREG */
    struct stat st;
    if (stat(link, &st) != 0) {
        printf("CONTRACT_FAIL stat: errno=%d\n", errno);
        return 1;
    }
    if (!S_ISREG(st.st_mode)) {
        printf("CONTRACT_FAIL stat_isreg: mode=0x%x\n", st.st_mode);
        return 1;
    }
    printf("stat_follow: ok\n");

    unlink(link);
    unlink(target);
    printf("CONTRACT_PASS\n");
    return 0;
}
