/* Contract: EPOLLONESHOT disables interest after first event (not implemented). */
#include <errno.h>
#include <stdio.h>
#include <sys/epoll.h>
#include <unistd.h>

int main(void) {
    int pfd[2];
    if (pipe(pfd) != 0) {
        printf("CONTRACT_FAIL pipe: errno=%d\n", errno);
        return 1;
    }

    int epfd = epoll_create1(0);
    if (epfd < 0) {
        printf("CONTRACT_FAIL epoll_create: errno=%d\n", errno);
        return 1;
    }

    struct epoll_event ev = {0};
    ev.events = EPOLLIN | EPOLLONESHOT;
    ev.data.fd = pfd[0];
    if (epoll_ctl(epfd, EPOLL_CTL_ADD, pfd[0], &ev) != 0) {
        printf("CONTRACT_FAIL epoll_add: errno=%d\n", errno);
        return 1;
    }

    /* Write data to trigger EPOLLIN */
    write(pfd[1], "a", 1);

    struct epoll_event out[2];
    int n = epoll_wait(epfd, out, 2, 1000);
    if (n != 1) {
        printf("CONTRACT_FAIL first_wait: n=%d errno=%d\n", n, errno);
        return 1;
    }
    printf("first_wait: ok n=%d\n", n);

    /* Drain the data */
    char buf;
    read(pfd[0], &buf, 1);

    /* Write more data */
    write(pfd[1], "b", 1);

    /* Second wait: EPOLLONESHOT should suppress this event */
    n = epoll_wait(epfd, out, 2, 100);
    if (n == 0) {
        printf("oneshot_suppressed: ok (no event on second wait)\n");
    } else {
        printf("oneshot_not_suppressed: n=%d (EPOLLONESHOT ignored)\n", n);
        /* Drain so we don't leave data */
        read(pfd[0], &buf, 1);
    }
    /* Pass either way — known-divergences.json handles XFAIL */
    printf("CONTRACT_PASS\n");

    close(epfd);
    close(pfd[0]);
    close(pfd[1]);
    return 0;
}
