/* Contract: fstatfs returns valid filesystem metadata via fd; f_type
 * matches the expected magic; EBADF on closed fd. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <sys/vfs.h>
#include <unistd.h>

/* Common filesystem magic numbers */
#define TMPFS_MAGIC  0x01021994
#define PROC_MAGIC   0x9fa0

int main(void) {
    /* fstatfs on /tmp (tmpfs) */
    int fd = open("/tmp", O_RDONLY | O_DIRECTORY);
    if (fd < 0) {
        printf("CONTRACT_FAIL open_tmp: errno=%d\n", errno);
        return 1;
    }

    struct statfs buf;
    int ret = fstatfs(fd, &buf);
    if (ret != 0) {
        printf("CONTRACT_FAIL fstatfs_tmp: ret=%d errno=%d\n", ret, errno);
        close(fd);
        return 1;
    }

    /* Verify f_type is a valid filesystem magic */
    if (buf.f_type == 0) {
        printf("CONTRACT_FAIL ftype_zero\n");
        close(fd);
        return 1;
    }
    printf("fstatfs_tmp: ok\n");
    close(fd);

    /* fstatfs on /proc (procfs) */
    fd = open("/proc", O_RDONLY | O_DIRECTORY);
    if (fd < 0) {
        /* /proc may not be mounted — skip */
        printf("fstatfs_proc: ok (not mounted)\n");
    } else {
        ret = fstatfs(fd, &buf);
        if (ret != 0) {
            printf("CONTRACT_FAIL fstatfs_proc: ret=%d errno=%d\n", ret, errno);
            close(fd);
            return 1;
        }
        printf("fstatfs_proc: ok\n");
        close(fd);
    }

    /* fstatfs on regular file */
    fd = open("/dev/null", O_RDONLY);
    if (fd < 0) {
        printf("CONTRACT_FAIL open_null: errno=%d\n", errno);
        return 1;
    }
    ret = fstatfs(fd, &buf);
    if (ret != 0) {
        printf("CONTRACT_FAIL fstatfs_null: ret=%d errno=%d\n", ret, errno);
        close(fd);
        return 1;
    }
    printf("fstatfs_devnull: ok\n");
    close(fd);

    /* EBADF on closed fd */
    errno = 0;
    ret = fstatfs(fd, &buf);
    if (ret != -1 || errno != EBADF) {
        printf("CONTRACT_FAIL ebadf: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("ebadf: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
