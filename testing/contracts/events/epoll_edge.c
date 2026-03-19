/* Contract: EPOLLET (edge-triggered) fires once per new event;
 * does not re-fire until data is consumed and new data arrives. */
#include <errno.h>
#include <stdio.h>
#include <sys/epoll.h>
#include <unistd.h>

int main(void) {
    int ep = epoll_create1(0);
    int fds[2];
    pipe(fds);

    struct epoll_event ev = {.events = EPOLLIN | EPOLLET, .data.fd = fds[0]};
    epoll_ctl(ep, EPOLL_CTL_ADD, fds[0], &ev);

    /* Write data */
    write(fds[1], "hello", 5);

    /* First wait: should fire */
    struct epoll_event out;
    int n = epoll_wait(ep, &out, 1, 100);
    if (n != 1) {
        printf("CONTRACT_FAIL first_wait: n=%d\n", n);
        return 1;
    }
    printf("first_wait: ok\n");

    /* Second wait without consuming: edge-triggered should NOT re-fire */
    n = epoll_wait(ep, &out, 1, 0);
    if (n != 0) {
        printf("CONTRACT_FAIL no_refire: n=%d\n", n);
        return 1;
    }
    printf("no_refire: ok\n");

    /* Consume all data */
    char buf[16];
    read(fds[0], buf, sizeof(buf));

    /* Write new data → edge fires again */
    write(fds[1], "world", 5);
    n = epoll_wait(ep, &out, 1, 100);
    if (n != 1) {
        printf("CONTRACT_FAIL rearm: n=%d\n", n);
        return 1;
    }
    printf("rearm: ok\n");

    read(fds[0], buf, sizeof(buf)); /* drain */
    close(fds[0]);
    close(fds[1]);
    close(ep);
    printf("CONTRACT_PASS\n");
    return 0;
}
