/* Contract: memfd_create returns usable anonymous fd; write + lseek + read
 * round-trips data; ftruncate works; fstat shows correct size. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

#ifndef MFD_CLOEXEC
#define MFD_CLOEXEC 0x0001U
#endif

static int my_memfd_create(const char *name, unsigned int flags) {
    return syscall(SYS_memfd_create, name, flags);
}

int main(void) {
    int fd = my_memfd_create("test", MFD_CLOEXEC);
    if (fd < 0) {
        printf("CONTRACT_FAIL memfd_create: errno=%d\n", errno);
        return 1;
    }
    printf("memfd_create: ok fd=%d\n", fd);

    /* Write data */
    const char *msg = "hello memfd";
    ssize_t n = write(fd, msg, strlen(msg));
    if (n != (ssize_t)strlen(msg)) {
        printf("CONTRACT_FAIL write: n=%ld errno=%d\n", (long)n, errno);
        return 1;
    }
    printf("write: ok\n");

    /* Seek back to start */
    off_t off = lseek(fd, 0, SEEK_SET);
    if (off != 0) {
        printf("CONTRACT_FAIL lseek: off=%ld errno=%d\n", (long)off, errno);
        return 1;
    }

    /* Read back */
    char buf[64] = {0};
    n = read(fd, buf, sizeof(buf));
    if (n != (ssize_t)strlen(msg) || memcmp(buf, msg, strlen(msg)) != 0) {
        printf("CONTRACT_FAIL read: n=%ld buf=%s\n", (long)n, buf);
        return 1;
    }
    printf("read_back: ok\n");

    /* fstat shows correct size */
    struct stat st;
    if (fstat(fd, &st) != 0) {
        printf("CONTRACT_FAIL fstat: errno=%d\n", errno);
        return 1;
    }
    if (st.st_size != (off_t)strlen(msg)) {
        printf("CONTRACT_FAIL fstat_size: got=%ld expected=%ld\n",
               (long)st.st_size, (long)strlen(msg));
        return 1;
    }
    printf("fstat_size: ok size=%ld\n", (long)st.st_size);

    /* ftruncate to larger */
    if (ftruncate(fd, 4096) != 0) {
        printf("CONTRACT_FAIL ftruncate: errno=%d\n", errno);
        return 1;
    }
    if (fstat(fd, &st) != 0 || st.st_size != 4096) {
        printf("CONTRACT_FAIL ftruncate_size: size=%ld\n", (long)st.st_size);
        return 1;
    }
    printf("ftruncate: ok\n");

    close(fd);
    printf("CONTRACT_PASS\n");
    return 0;
}
