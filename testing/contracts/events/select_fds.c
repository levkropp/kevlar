/* Contract: select() on pipe with timeout; write makes read-end readable;
 * close write → read-end readable (EOF). */
#include <errno.h>
#include <stdio.h>
#include <sys/select.h>
#include <unistd.h>

int main(void) {
    int fds[2];
    pipe(fds);

    fd_set rfds, wfds;
    struct timeval tv;

    /* Read-end not ready */
    FD_ZERO(&rfds);
    FD_SET(fds[0], &rfds);
    tv.tv_sec = 0;
    tv.tv_usec = 0;
    int ret = select(fds[0] + 1, &rfds, NULL, NULL, &tv);
    if (ret != 0) {
        printf("CONTRACT_FAIL empty_select: ret=%d\n", ret);
        return 1;
    }
    printf("empty_select: ok\n");

    /* Write-end ready for writing */
    FD_ZERO(&wfds);
    FD_SET(fds[1], &wfds);
    tv.tv_sec = 0;
    tv.tv_usec = 0;
    ret = select(fds[1] + 1, NULL, &wfds, NULL, &tv);
    if (ret != 1 || !FD_ISSET(fds[1], &wfds)) {
        printf("CONTRACT_FAIL write_ready: ret=%d\n", ret);
        return 1;
    }
    printf("write_ready: ok\n");

    /* Write data → read-end becomes readable */
    write(fds[1], "hi", 2);
    FD_ZERO(&rfds);
    FD_SET(fds[0], &rfds);
    tv.tv_sec = 0;
    tv.tv_usec = 0;
    ret = select(fds[0] + 1, &rfds, NULL, NULL, &tv);
    if (ret != 1 || !FD_ISSET(fds[0], &rfds)) {
        printf("CONTRACT_FAIL read_ready: ret=%d\n", ret);
        return 1;
    }
    printf("read_ready: ok\n");
    char buf[4];
    read(fds[0], buf, 2); /* drain */

    /* Close write → read-end readable (EOF) */
    close(fds[1]);
    FD_ZERO(&rfds);
    FD_SET(fds[0], &rfds);
    tv.tv_sec = 0;
    tv.tv_usec = 0;
    ret = select(fds[0] + 1, &rfds, NULL, NULL, &tv);
    if (ret != 1 || !FD_ISSET(fds[0], &rfds)) {
        printf("CONTRACT_FAIL eof_readable: ret=%d\n", ret);
        return 1;
    }
    printf("eof_readable: ok\n");

    close(fds[0]);
    printf("CONTRACT_PASS\n");
    return 0;
}
