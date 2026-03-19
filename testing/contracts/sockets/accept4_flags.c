/* Contract: accept4 applies SOCK_NONBLOCK and SOCK_CLOEXEC to accepted fd. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <unistd.h>

int main(void) {
    const char *path = "/tmp/accept4_test.sock";
    unlink(path);

    int srv = socket(AF_UNIX, SOCK_STREAM, 0);
    if (srv < 0) {
        printf("CONTRACT_FAIL socket: errno=%d\n", errno);
        return 1;
    }

    struct sockaddr_un addr = {0};
    addr.sun_family = AF_UNIX;
    strncpy(addr.sun_path, path, sizeof(addr.sun_path) - 1);

    if (bind(srv, (struct sockaddr *)&addr, sizeof(addr)) != 0) {
        printf("CONTRACT_FAIL bind: errno=%d\n", errno);
        return 1;
    }
    if (listen(srv, 1) != 0) {
        printf("CONTRACT_FAIL listen: errno=%d\n", errno);
        return 1;
    }

    /* Fork child to connect */
    pid_t child = fork();
    if (child == 0) {
        close(srv);
        int c = socket(AF_UNIX, SOCK_STREAM, 0);
        connect(c, (struct sockaddr *)&addr, sizeof(addr));
        write(c, "hi", 2);
        close(c);
        _exit(0);
    }

    /* accept4 with SOCK_NONBLOCK */
    int client = accept4(srv, NULL, NULL, SOCK_NONBLOCK);
    if (client < 0) {
        printf("CONTRACT_FAIL accept4_nb: errno=%d\n", errno);
        return 1;
    }
    int flags = fcntl(client, F_GETFL);
    if (!(flags & O_NONBLOCK)) {
        printf("CONTRACT_FAIL nonblock: flags=0x%x missing O_NONBLOCK\n", flags);
        close(client);
        return 1;
    }
    printf("accept4_nonblock: ok flags=0x%x\n", flags);
    close(client);

    /* Wait for child, then test SOCK_CLOEXEC */
    waitpid(child, NULL, 0);

    /* New connection for CLOEXEC test */
    child = fork();
    if (child == 0) {
        close(srv);
        int c = socket(AF_UNIX, SOCK_STREAM, 0);
        connect(c, (struct sockaddr *)&addr, sizeof(addr));
        write(c, "hi", 2);
        close(c);
        _exit(0);
    }

    client = accept4(srv, NULL, NULL, SOCK_CLOEXEC);
    if (client < 0) {
        printf("CONTRACT_FAIL accept4_cloexec: errno=%d\n", errno);
        return 1;
    }
    int fdflags = fcntl(client, F_GETFD);
    if (!(fdflags & FD_CLOEXEC)) {
        printf("CONTRACT_FAIL cloexec: fdflags=0x%x missing FD_CLOEXEC\n", fdflags);
        close(client);
        return 1;
    }
    printf("accept4_cloexec: ok fdflags=0x%x\n", fdflags);
    close(client);

    waitpid(child, NULL, 0);
    close(srv);
    unlink(path);
    printf("CONTRACT_PASS\n");
    return 0;
}
