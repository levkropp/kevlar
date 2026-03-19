/* Contract: getsockopt SO_TYPE, SO_ERROR; setsockopt SO_REUSEADDR. */
#include <errno.h>
#include <stdio.h>
#include <sys/socket.h>
#include <unistd.h>

int main(void) {
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0) {
        printf("CONTRACT_FAIL socket: errno=%d\n", errno);
        return 1;
    }

    /* SO_TYPE returns SOCK_STREAM */
    int type;
    socklen_t len = sizeof(type);
    if (getsockopt(fd, SOL_SOCKET, SO_TYPE, &type, &len) != 0) {
        printf("CONTRACT_FAIL getsockopt_type: errno=%d\n", errno);
        return 1;
    }
    if (type != SOCK_STREAM) {
        printf("CONTRACT_FAIL so_type: got=%d expected=%d\n", type, SOCK_STREAM);
        return 1;
    }
    printf("so_type: ok\n");

    /* SO_ERROR = 0 on fresh socket */
    int err;
    len = sizeof(err);
    if (getsockopt(fd, SOL_SOCKET, SO_ERROR, &err, &len) != 0) {
        printf("CONTRACT_FAIL getsockopt_error: errno=%d\n", errno);
        return 1;
    }
    if (err != 0) {
        printf("CONTRACT_FAIL so_error: got=%d\n", err);
        return 1;
    }
    printf("so_error: ok\n");

    close(fd);

    /* SOCK_DGRAM */
    int dg = socket(AF_UNIX, SOCK_DGRAM, 0);
    if (dg < 0) {
        printf("CONTRACT_FAIL socket_dgram: errno=%d\n", errno);
        return 1;
    }
    len = sizeof(type);
    getsockopt(dg, SOL_SOCKET, SO_TYPE, &type, &len);
    if (type != SOCK_DGRAM) {
        printf("CONTRACT_FAIL so_type_dgram: got=%d expected=%d\n", type, SOCK_DGRAM);
        return 1;
    }
    printf("so_type_dgram: ok\n");

    /* SO_REUSEADDR: setsockopt should at least not fail */
    int val = 1;
    if (setsockopt(dg, SOL_SOCKET, SO_REUSEADDR, &val, sizeof(val)) != 0) {
        printf("CONTRACT_FAIL setsockopt_reuseaddr: errno=%d\n", errno);
        return 1;
    }
    printf("so_reuseaddr_set: ok\n");

    close(dg);
    printf("CONTRACT_PASS\n");
    return 0;
}
