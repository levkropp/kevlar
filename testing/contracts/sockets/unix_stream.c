/* Contract: AF_UNIX stream bind/listen/connect/accept data exchange. */
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <unistd.h>

int main(void) {
    const char *path = "/tmp/contract_unix.sock";
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
    printf("bind_listen: ok\n");

    pid_t pid = fork();
    if (pid == 0) {
        /* Child: connect and send */
        close(srv);
        int cli = socket(AF_UNIX, SOCK_STREAM, 0);
        if (connect(cli, (struct sockaddr *)&addr, sizeof(addr)) != 0) {
            printf("CONTRACT_FAIL connect: errno=%d\n", errno);
            _exit(1);
        }
        write(cli, "hello", 5);
        char buf[16] = {0};
        int n = read(cli, buf, sizeof(buf));
        if (n != 5 || memcmp(buf, "world", 5) != 0) {
            printf("CONTRACT_FAIL child_read: n=%d buf=%s\n", n, buf);
            _exit(1);
        }
        close(cli);
        _exit(0);
    }

    /* Parent: accept and exchange */
    int conn = accept(srv, NULL, NULL);
    if (conn < 0) {
        printf("CONTRACT_FAIL accept: errno=%d\n", errno);
        return 1;
    }
    char buf[16] = {0};
    int n = read(conn, buf, sizeof(buf));
    if (n != 5 || memcmp(buf, "hello", 5) != 0) {
        printf("CONTRACT_FAIL server_read: n=%d buf=%s\n", n, buf);
        return 1;
    }
    write(conn, "world", 5);
    printf("data_exchange: ok\n");

    int status;
    waitpid(pid, &status, 0);
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        printf("CONTRACT_FAIL child_exit: status=%d\n", status);
        return 1;
    }
    printf("child_ok: ok\n");

    close(conn);
    close(srv);
    unlink(path);
    printf("CONTRACT_PASS\n");
    return 0;
}
