/* Contract: mmap with a file descriptor maps the file contents into memory.
   Reads from the mapped region should return file data. */
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

int main(void) {
    /* Create a temporary file with known content */
    const char *path = "/tmp/mmap_test";
    int fd = open(path, O_RDWR | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) {
        printf("CONTRACT_FAIL create\n");
        return 1;
    }

    /* Write known pattern: 4096 bytes (one page) */
    char buf[4096];
    for (int i = 0; i < 4096; i++)
        buf[i] = (char)(i & 0xFF);

    int nw = write(fd, buf, sizeof(buf));
    if (nw != sizeof(buf)) {
        printf("CONTRACT_FAIL write (got %d)\n", nw);
        close(fd);
        return 1;
    }

    /* mmap the file for reading */
    char *p = mmap(NULL, sizeof(buf), PROT_READ, MAP_PRIVATE, fd, 0);
    if (p == MAP_FAILED) {
        printf("CONTRACT_FAIL mmap\n");
        close(fd);
        return 1;
    }

    /* Compare mapped contents to original buffer */
    if (memcmp(p, buf, sizeof(buf)) != 0) {
        for (int i = 0; i < (int)sizeof(buf); i++) {
            if (p[i] != buf[i]) {
                printf("CONTRACT_FAIL content_mismatch at byte %d (mmap=%d expected=%d)\n",
                       i, (unsigned char)p[i], (unsigned char)buf[i]);
                break;
            }
        }
        munmap(p, sizeof(buf));
        close(fd);
        return 1;
    }
    printf("content_match: ok (4096 bytes)\n");

    munmap(p, sizeof(buf));
    close(fd);
    unlink(path);
    printf("CONTRACT_PASS\n");
    return 0;
}
