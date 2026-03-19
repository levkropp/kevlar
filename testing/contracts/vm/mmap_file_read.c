/* Contract: MAP_PRIVATE file mmap reads file content;
 * write to MAP_PRIVATE doesn't modify underlying file. */
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

int main(void) {
    const char *path = "/tmp/contract_mmap_file";
    unlink(path);

    /* Create file with known content */
    int fd = open(path, O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (fd < 0) {
        printf("CONTRACT_FAIL open\n");
        return 1;
    }
    const char *data = "mmap test data!!"; /* 16 bytes */
    write(fd, data, 16);

    /* MAP_PRIVATE: content matches file */
    char *p = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE, fd, 0);
    if (p == MAP_FAILED) {
        printf("CONTRACT_FAIL mmap\n");
        return 1;
    }
    if (memcmp(p, data, 16) != 0) {
        printf("CONTRACT_FAIL content: got=%.16s\n", p);
        return 1;
    }
    printf("mmap_content: ok\n");

    /* Write to MAP_PRIVATE page → copy-on-write, file unchanged */
    p[0] = 'X';
    if (p[0] != 'X') {
        printf("CONTRACT_FAIL private_write: got=%c\n", p[0]);
        return 1;
    }

    /* Verify file unchanged by reading it again */
    char buf[16] = {0};
    lseek(fd, 0, SEEK_SET);
    read(fd, buf, 16);
    if (buf[0] != 'm') {
        printf("CONTRACT_FAIL file_unchanged: got=%c\n", buf[0]);
        return 1;
    }
    printf("private_cow: ok\n");

    munmap(p, 4096);
    close(fd);
    unlink(path);
    printf("CONTRACT_PASS\n");
    return 0;
}
