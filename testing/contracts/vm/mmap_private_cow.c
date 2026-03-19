/* Contract: MAP_PRIVATE is copy-on-write; writes do not affect file or other mappings. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

int main(void) {
    const char *path = "/tmp/cow_test";
    int fd = open(path, O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (fd < 0) {
        printf("CONTRACT_FAIL open: errno=%d\n", errno);
        return 1;
    }
    write(fd, "AAAA", 4);

    /* First MAP_PRIVATE mapping */
    void *m1 = mmap(NULL, 4096, PROT_READ | PROT_WRITE,
                     MAP_PRIVATE, fd, 0);
    if (m1 == MAP_FAILED) {
        printf("CONTRACT_FAIL mmap1: errno=%d\n", errno);
        return 1;
    }
    if (memcmp(m1, "AAAA", 4) != 0) {
        printf("CONTRACT_FAIL mmap1_data: got='%.4s'\n", (char *)m1);
        return 1;
    }
    printf("mmap1: ok data='%.4s'\n", (char *)m1);

    /* COW write to m1 */
    ((char *)m1)[0] = 'B';
    printf("cow_write: ok m1[0]='%c'\n", ((char *)m1)[0]);

    /* Second MAP_PRIVATE mapping must still see 'A' */
    void *m2 = mmap(NULL, 4096, PROT_READ | PROT_WRITE,
                     MAP_PRIVATE, fd, 0);
    if (m2 == MAP_FAILED) {
        printf("CONTRACT_FAIL mmap2: errno=%d\n", errno);
        return 1;
    }
    if (((char *)m2)[0] != 'A') {
        printf("CONTRACT_FAIL mmap2_data: m2[0]='%c' expected='A'\n", ((char *)m2)[0]);
        return 1;
    }
    printf("mmap2_independent: ok m2[0]='%c'\n", ((char *)m2)[0]);

    /* Underlying file must be unchanged */
    char buf[4] = {0};
    pread(fd, buf, 4, 0);
    if (buf[0] != 'A') {
        printf("CONTRACT_FAIL file_unchanged: buf[0]='%c' expected='A'\n", buf[0]);
        return 1;
    }
    printf("file_unchanged: ok\n");

    munmap(m1, 4096);
    munmap(m2, 4096);
    close(fd);
    unlink(path);
    printf("CONTRACT_PASS\n");
    return 0;
}
