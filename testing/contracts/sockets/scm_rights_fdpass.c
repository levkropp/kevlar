/* Contract: sendmsg/recvmsg SCM_RIGHTS passes file descriptors. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

int main(void) {
    /* Create a file with known content */
    const char *path = "/tmp/scm_test";
    int fd = open(path, O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (fd < 0) {
        printf("CONTRACT_FAIL open: errno=%d\n", errno);
        return 1;
    }
    write(fd, "secret", 6);

    /* socketpair */
    int sv[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) != 0) {
        printf("CONTRACT_FAIL socketpair: errno=%d\n", errno);
        return 1;
    }

    /* Send fd via SCM_RIGHTS on sv[0] */
    char data = 'F';
    struct iovec iov = {.iov_base = &data, .iov_len = 1};

    union {
        char buf[CMSG_SPACE(sizeof(int))];
        struct cmsghdr align;
    } cmsg_buf;
    memset(&cmsg_buf, 0, sizeof(cmsg_buf));

    struct msghdr msg = {0};
    msg.msg_iov = &iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_buf.buf;
    msg.msg_controllen = sizeof(cmsg_buf.buf);

    struct cmsghdr *cmsg = CMSG_FIRSTHDR(&msg);
    cmsg->cmsg_level = SOL_SOCKET;
    cmsg->cmsg_type = SCM_RIGHTS;
    cmsg->cmsg_len = CMSG_LEN(sizeof(int));
    memcpy(CMSG_DATA(cmsg), &fd, sizeof(int));

    if (sendmsg(sv[0], &msg, 0) < 0) {
        printf("CONTRACT_FAIL sendmsg: errno=%d\n", errno);
        return 1;
    }
    printf("sendmsg: ok\n");
    close(fd); /* close original */

    /* Receive fd via SCM_RIGHTS on sv[1] */
    char recv_data = 0;
    struct iovec recv_iov = {.iov_base = &recv_data, .iov_len = 1};

    union {
        char buf[CMSG_SPACE(sizeof(int))];
        struct cmsghdr align;
    } recv_cmsg_buf;
    memset(&recv_cmsg_buf, 0, sizeof(recv_cmsg_buf));

    struct msghdr recv_msg = {0};
    recv_msg.msg_iov = &recv_iov;
    recv_msg.msg_iovlen = 1;
    recv_msg.msg_control = recv_cmsg_buf.buf;
    recv_msg.msg_controllen = sizeof(recv_cmsg_buf.buf);

    if (recvmsg(sv[1], &recv_msg, 0) < 0) {
        printf("CONTRACT_FAIL recvmsg: errno=%d\n", errno);
        return 1;
    }
    if (recv_data != 'F') {
        printf("CONTRACT_FAIL recv_data: got='%c'\n", recv_data);
        return 1;
    }
    printf("recvmsg: ok data='%c'\n", recv_data);

    /* Extract the passed fd */
    cmsg = CMSG_FIRSTHDR(&recv_msg);
    if (cmsg == NULL || cmsg->cmsg_level != SOL_SOCKET ||
        cmsg->cmsg_type != SCM_RIGHTS) {
        printf("CONTRACT_FAIL cmsg: null or wrong type\n");
        return 1;
    }
    int new_fd;
    memcpy(&new_fd, CMSG_DATA(cmsg), sizeof(int));
    printf("fd_received: ok new_fd=%d\n", new_fd);

    /* Seek to 0 and read content from passed fd */
    lseek(new_fd, 0, SEEK_SET);
    char buf[16] = {0};
    ssize_t n = read(new_fd, buf, sizeof(buf));
    if (n != 6 || memcmp(buf, "secret", 6) != 0) {
        printf("CONTRACT_FAIL read_content: n=%ld buf='%s'\n", (long)n, buf);
        return 1;
    }
    printf("content_verified: ok\n");

    close(new_fd);
    close(sv[0]);
    close(sv[1]);
    unlink(path);
    printf("CONTRACT_PASS\n");
    return 0;
}
