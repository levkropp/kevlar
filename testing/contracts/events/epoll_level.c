/* Contract: level-triggered epoll re-fires while data available;
 * CTL_DEL removes; CTL_MOD changes events. */
#include <errno.h>
#include <stdio.h>
#include <sys/epoll.h>
#include <unistd.h>

int main(void) {
    int ep = epoll_create1(0);
    if (ep < 0) {
        printf("CONTRACT_FAIL epoll_create1: errno=%d\n", errno);
        return 1;
    }

    int fds[2];
    pipe(fds);

    struct epoll_event ev = {.events = EPOLLIN, .data.fd = fds[0]};
    epoll_ctl(ep, EPOLL_CTL_ADD, fds[0], &ev);

    /* No data yet — timeout immediately */
    struct epoll_event out;
    int n = epoll_wait(ep, &out, 1, 0);
    if (n != 0) {
        printf("CONTRACT_FAIL empty_wait: n=%d\n", n);
        return 1;
    }
    printf("empty_wait: ok\n");

    /* Write data → EPOLLIN */
    write(fds[1], "abc", 3);
    n = epoll_wait(ep, &out, 1, 100);
    if (n != 1 || !(out.events & EPOLLIN)) {
        printf("CONTRACT_FAIL epollin: n=%d events=0x%x\n", n, out.events);
        return 1;
    }
    printf("epollin: ok\n");

    /* Partial read — level-triggered should still fire */
    char buf[2];
    read(fds[0], buf, 1);
    n = epoll_wait(ep, &out, 1, 0);
    if (n != 1) {
        printf("CONTRACT_FAIL level_refire: n=%d\n", n);
        return 1;
    }
    printf("level_refire: ok\n");

    /* Drain completely — should not fire */
    read(fds[0], buf, 2);
    n = epoll_wait(ep, &out, 1, 0);
    if (n != 0) {
        printf("CONTRACT_FAIL drained: n=%d\n", n);
        return 1;
    }
    printf("drained: ok\n");

    /* CTL_DEL: remove fd */
    epoll_ctl(ep, EPOLL_CTL_DEL, fds[0], NULL);
    write(fds[1], "x", 1);
    n = epoll_wait(ep, &out, 1, 0);
    if (n != 0) {
        printf("CONTRACT_FAIL after_del: n=%d\n", n);
        return 1;
    }
    printf("ctl_del: ok\n");
    read(fds[0], buf, 1); /* drain */

    /* CTL_ADD again with EPOLLOUT */
    ev.events = EPOLLOUT;
    ev.data.fd = fds[1];
    epoll_ctl(ep, EPOLL_CTL_ADD, fds[1], &ev);
    n = epoll_wait(ep, &out, 1, 0);
    if (n != 1 || !(out.events & EPOLLOUT)) {
        printf("CONTRACT_FAIL epollout: n=%d events=0x%x\n", n, out.events);
        return 1;
    }
    printf("epollout: ok\n");

    close(fds[0]);
    close(fds[1]);
    close(ep);
    printf("CONTRACT_PASS\n");
    return 0;
}
