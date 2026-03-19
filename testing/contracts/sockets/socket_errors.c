/* Contract: socket() with invalid domain returns EAFNOSUPPORT;
 * invalid type within valid domain returns EINVAL. */
#include <errno.h>
#include <stdio.h>
#include <sys/socket.h>
#include <unistd.h>

int main(void) {
    /* Invalid address family → EAFNOSUPPORT */
    errno = 0;
    int fd = socket(9999, SOCK_STREAM, 0);
    if (fd != -1) {
        printf("CONTRACT_FAIL bad_family: fd=%d\n", fd);
        close(fd);
        return 1;
    }
    if (errno != EAFNOSUPPORT) {
        printf("CONTRACT_FAIL bad_family_errno: errno=%d expected=%d\n", errno, EAFNOSUPPORT);
        return 1;
    }
    printf("bad_family: ok\n");

    /* Invalid socket type within valid family → EINVAL */
    errno = 0;
    fd = socket(AF_UNIX, 9999, 0);
    if (fd != -1) {
        printf("CONTRACT_FAIL bad_type: fd=%d\n", fd);
        close(fd);
        return 1;
    }
    if (errno != EINVAL) {
        printf("CONTRACT_FAIL bad_type_errno: errno=%d expected=%d\n", errno, EINVAL);
        return 1;
    }
    printf("bad_type: ok\n");

    /* Valid socket creation */
    fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0) {
        printf("CONTRACT_FAIL valid_socket: errno=%d\n", errno);
        return 1;
    }
    printf("valid_socket: ok\n");
    close(fd);

    printf("CONTRACT_PASS\n");
    return 0;
}
