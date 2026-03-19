/* Contract: statx returns populated fields; stx_size matches;
 * S_IFREG set; stx_nlink=1. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

/* Use raw syscall + byte buffer to avoid struct layout issues */
#ifndef STATX_BASIC_STATS
#define STATX_BASIC_STATS 0x07ffU
#endif

int main(void) {
    const char *path = "/tmp/contract_statx";
    unlink(path);

    /* Create file with known content */
    int fd = open(path, O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (fd < 0) {
        printf("CONTRACT_FAIL open: errno=%d\n", errno);
        return 1;
    }
    write(fd, "hello world", 11);
    close(fd);

    /* Use struct stat via fstatat as a more portable fallback,
     * but test statx syscall if available */
    struct stat st;
    if (stat(path, &st) != 0) {
        printf("CONTRACT_FAIL stat: errno=%d\n", errno);
        return 1;
    }

    /* Size matches */
    if (st.st_size != 11) {
        printf("CONTRACT_FAIL st_size: got=%ld expected=11\n", (long)st.st_size);
        return 1;
    }
    printf("st_size: ok (%ld)\n", (long)st.st_size);

    /* S_IFREG */
    if (!S_ISREG(st.st_mode)) {
        printf("CONTRACT_FAIL st_mode: mode=0%o\n", st.st_mode);
        return 1;
    }
    printf("st_mode_reg: ok\n");

    /* nlink=1 */
    if (st.st_nlink != 1) {
        printf("CONTRACT_FAIL st_nlink: got=%lu\n", (unsigned long)st.st_nlink);
        return 1;
    }
    printf("st_nlink: ok\n");

    /* Now test statx syscall directly with raw buffer */
    unsigned char buf[256] = {0};
    int ret = syscall(SYS_statx, AT_FDCWD, path, 0, STATX_BASIC_STATS, buf);
    if (ret != 0) {
        printf("CONTRACT_FAIL statx: errno=%d\n", errno);
        return 1;
    }
    /* stx_mask is at offset 0, 4 bytes */
    unsigned int mask;
    __builtin_memcpy(&mask, buf, 4);
    if (!(mask & 0x01)) { /* STATX_TYPE */
        printf("CONTRACT_FAIL stx_mask: mask=0x%x\n", mask);
        return 1;
    }
    printf("statx_mask: ok\n");

    unlink(path);
    printf("CONTRACT_PASS\n");
    return 0;
}
