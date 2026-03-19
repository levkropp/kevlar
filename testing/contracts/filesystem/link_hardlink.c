/* Contract: hard link shares inode; st_nlink=2;
 * unlink original, link still accessible. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

int main(void) {
    const char *orig = "/tmp/contract_hl_orig";
    const char *hard = "/tmp/contract_hl_link";
    unlink(orig);
    unlink(hard);

    /* Create original */
    int fd = open(orig, O_CREAT | O_WRONLY | O_TRUNC, 0644);
    write(fd, "content", 7);
    close(fd);

    /* Create hard link */
    if (link(orig, hard) != 0) {
        printf("CONTRACT_FAIL link: errno=%d\n", errno);
        return 1;
    }

    /* Same inode */
    struct stat s1, s2;
    stat(orig, &s1);
    stat(hard, &s2);
    if (s1.st_ino != s2.st_ino) {
        printf("CONTRACT_FAIL same_ino: orig=%lu hard=%lu\n",
               (unsigned long)s1.st_ino, (unsigned long)s2.st_ino);
        return 1;
    }
    printf("same_ino: ok\n");

    /* nlink=2 */
    if (s1.st_nlink != 2) {
        printf("CONTRACT_FAIL nlink: got=%lu\n", (unsigned long)s1.st_nlink);
        return 1;
    }
    printf("nlink: ok\n");

    /* Unlink original, hard link still accessible */
    unlink(orig);
    fd = open(hard, O_RDONLY);
    if (fd < 0) {
        printf("CONTRACT_FAIL hard_after_unlink: errno=%d\n", errno);
        return 1;
    }
    char buf[16] = {0};
    read(fd, buf, 7);
    close(fd);
    if (memcmp(buf, "content", 7) != 0) {
        printf("CONTRACT_FAIL hard_content: buf=%s\n", buf);
        return 1;
    }
    printf("hard_after_unlink: ok\n");

    /* nlink now 1 */
    stat(hard, &s2);
    if (s2.st_nlink != 1) {
        printf("CONTRACT_FAIL nlink_after: got=%lu\n", (unsigned long)s2.st_nlink);
        return 1;
    }
    printf("nlink_after: ok\n");

    unlink(hard);
    printf("CONTRACT_PASS\n");
    return 0;
}
