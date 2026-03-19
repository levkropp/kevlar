/* Contract: statfs/fstatfs return correct magic for tmpfs and procfs. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <sys/statfs.h>
#include <unistd.h>

#define TMPFS_MAGIC  0x01021994
#define PROC_MAGIC   0x9FA0

int main(void) {
    struct statfs sfs;

    /* statfs on /tmp → TMPFS_MAGIC */
    if (statfs("/tmp", &sfs) != 0) {
        printf("CONTRACT_FAIL statfs_tmp: errno=%d\n", errno);
        return 1;
    }
    if ((unsigned long)sfs.f_type != TMPFS_MAGIC) {
        printf("CONTRACT_FAIL tmp_magic: got=0x%lx expected=0x%x\n",
               (unsigned long)sfs.f_type, TMPFS_MAGIC);
        return 1;
    }
    if (sfs.f_bsize == 0) {
        printf("CONTRACT_FAIL tmp_bsize: got=0\n");
        return 1;
    }
    printf("statfs_tmp: ok magic=0x%lx bsize=%ld\n",
           (unsigned long)sfs.f_type, (long)sfs.f_bsize);

    /* fstatfs on an open fd in /tmp */
    int fd = open("/tmp/statfs_probe", O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (fd < 0) {
        printf("CONTRACT_FAIL open: errno=%d\n", errno);
        return 1;
    }
    struct statfs sfs2;
    if (fstatfs(fd, &sfs2) != 0) {
        printf("CONTRACT_FAIL fstatfs: errno=%d\n", errno);
        close(fd);
        return 1;
    }
    if ((unsigned long)sfs2.f_type != TMPFS_MAGIC) {
        printf("CONTRACT_FAIL fstatfs_magic: got=0x%lx\n", (unsigned long)sfs2.f_type);
        close(fd);
        return 1;
    }
    close(fd);
    unlink("/tmp/statfs_probe");
    printf("fstatfs: ok\n");

    /* statfs on /proc → PROC_SUPER_MAGIC */
    if (statfs("/proc", &sfs) != 0) {
        printf("CONTRACT_FAIL statfs_proc: errno=%d\n", errno);
        return 1;
    }
    if ((unsigned long)sfs.f_type != PROC_MAGIC) {
        printf("CONTRACT_FAIL proc_magic: got=0x%lx expected=0x%x\n",
               (unsigned long)sfs.f_type, PROC_MAGIC);
        return 1;
    }
    printf("statfs_proc: ok magic=0x%lx\n", (unsigned long)sfs.f_type);

    printf("CONTRACT_PASS\n");
    return 0;
}
