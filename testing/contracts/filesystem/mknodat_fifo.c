/* Contract: mknod with S_IFIFO accepted (stub returns 0). */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <sys/stat.h>
#include <unistd.h>

int main(void) {
    const char *path = "/tmp/test_fifo";
    unlink(path);

    /* mknod S_IFIFO → 0 (stub accepts FIFOs) */
    if (mknod(path, S_IFIFO | 0644, 0) != 0) {
        printf("CONTRACT_FAIL mknod_fifo: errno=%d\n", errno);
        return 1;
    }
    printf("mknod_fifo: ok\n");

    unlink(path);
    printf("CONTRACT_PASS\n");
    return 0;
}
