/* Contract: getdents64 returns directory entries including "." and "..";
 * created files appear; d_type is correct. */
#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

struct linux_dirent64 {
    unsigned long long d_ino;
    long long d_off;
    unsigned short d_reclen;
    unsigned char d_type;
    char d_name[];
};

int main(void) {
    const char *dir = "/tmp/contract_getdents";
    rmdir(dir);
    mkdir(dir, 0755);

    /* Create a file inside */
    char path[256];
    snprintf(path, sizeof(path), "%s/testfile", dir);
    int fd = open(path, O_CREAT | O_WRONLY, 0644);
    close(fd);

    /* Open directory and read entries */
    int dfd = open(dir, O_RDONLY | O_DIRECTORY);
    if (dfd < 0) {
        printf("CONTRACT_FAIL open_dir: errno=%d\n", errno);
        return 1;
    }

    char buf[1024];
    int nread = syscall(SYS_getdents64, dfd, buf, sizeof(buf));
    if (nread <= 0) {
        printf("CONTRACT_FAIL getdents64: nread=%d errno=%d\n", nread, errno);
        return 1;
    }

    int found_dot = 0, found_dotdot = 0, found_file = 0;
    int pos = 0;
    while (pos < nread) {
        struct linux_dirent64 *d = (struct linux_dirent64 *)(buf + pos);
        if (strcmp(d->d_name, ".") == 0) found_dot = 1;
        else if (strcmp(d->d_name, "..") == 0) found_dotdot = 1;
        else if (strcmp(d->d_name, "testfile") == 0) {
            found_file = 1;
            if (d->d_type != DT_REG) {
                printf("CONTRACT_FAIL d_type: got=%d expected=%d\n", d->d_type, DT_REG);
                return 1;
            }
        }
        pos += d->d_reclen;
    }

    if (!found_dot) {
        printf("CONTRACT_FAIL missing_dot\n");
        return 1;
    }
    if (!found_dotdot) {
        printf("CONTRACT_FAIL missing_dotdot\n");
        return 1;
    }
    if (!found_file) {
        printf("CONTRACT_FAIL missing_testfile\n");
        return 1;
    }
    printf("getdents64: ok (dot=%d dotdot=%d file=%d)\n", found_dot, found_dotdot, found_file);

    close(dfd);
    unlink(path);
    rmdir(dir);
    printf("CONTRACT_PASS\n");
    return 0;
}
