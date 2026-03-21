/* Contract: getsockname returns bound address on unix socket;
 * getpeername returns remote address on connected socketpair. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

int main(void) {
    /* socketpair: both ends are connected, so getpeername should work */
    int fds[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, fds) != 0) {
        printf("CONTRACT_FAIL socketpair: errno=%d\n", errno);
        return 1;
    }

    /* getsockname on socketpair — returns AF_UNIX with empty path */
    struct sockaddr_un addr;
    socklen_t len = sizeof(addr);
    memset(&addr, 0xff, sizeof(addr));
    int ret = getsockname(fds[0], (struct sockaddr *)&addr, &len);
    if (ret != 0) {
        printf("CONTRACT_FAIL getsockname: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    if (addr.sun_family != AF_UNIX) {
        printf("CONTRACT_FAIL getsockname_family: got=%d expected=%d\n",
               addr.sun_family, AF_UNIX);
        return 1;
    }
    printf("getsockname: ok family=%d\n", addr.sun_family);

    /* getpeername on socketpair */
    len = sizeof(addr);
    memset(&addr, 0xff, sizeof(addr));
    ret = getpeername(fds[0], (struct sockaddr *)&addr, &len);
    if (ret != 0) {
        printf("CONTRACT_FAIL getpeername: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    if (addr.sun_family != AF_UNIX) {
        printf("CONTRACT_FAIL getpeername_family: got=%d expected=%d\n",
               addr.sun_family, AF_UNIX);
        return 1;
    }
    printf("getpeername: ok family=%d\n", addr.sun_family);

    /* getpeername on unconnected socket should fail with ENOTCONN */
    int sock = socket(AF_UNIX, SOCK_STREAM, 0);
    if (sock < 0) {
        printf("CONTRACT_FAIL socket: errno=%d\n", errno);
        return 1;
    }
    len = sizeof(addr);
    errno = 0;
    ret = getpeername(sock, (struct sockaddr *)&addr, &len);
    if (ret != -1 || errno != ENOTCONN) {
        printf("CONTRACT_FAIL enotconn: ret=%d errno=%d\n", ret, errno);
        close(sock);
        return 1;
    }
    printf("enotconn: ok\n");

    close(sock);
    close(fds[0]);
    close(fds[1]);
    printf("CONTRACT_PASS\n");
    return 0;
}
