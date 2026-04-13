// LD_PRELOAD wrapper to trace socket-related libc calls.
// gcc -shared -fPIC -o trace_sock.so trace_sock.c -ldl
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdio.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <errno.h>
#include <unistd.h>
#include <fcntl.h>

static int (*real_socket)(int, int, int);
static int (*real_bind)(int, const struct sockaddr *, socklen_t);
static int (*real_connect)(int, const struct sockaddr *, socklen_t);
static ssize_t (*real_sendto)(int, const void *, size_t, int, const struct sockaddr *, socklen_t);
static ssize_t (*real_recvfrom)(int, void *, size_t, int, struct sockaddr *, socklen_t *);
static int (*real_setsockopt)(int, int, int, const void *, socklen_t);
static int (*real_getsockopt)(int, int, int, void *, socklen_t *);
static int (*real_getsockname)(int, struct sockaddr *, socklen_t *);
static int (*real_fcntl)(int, int, ...);
static int (*real_close)(int);

static void init(void) {
    real_socket = dlsym(RTLD_NEXT, "socket");
    real_bind = dlsym(RTLD_NEXT, "bind");
    real_connect = dlsym(RTLD_NEXT, "connect");
    real_sendto = dlsym(RTLD_NEXT, "sendto");
    real_recvfrom = dlsym(RTLD_NEXT, "recvfrom");
    real_setsockopt = dlsym(RTLD_NEXT, "setsockopt");
    real_getsockopt = dlsym(RTLD_NEXT, "getsockopt");
    real_getsockname = dlsym(RTLD_NEXT, "getsockname");
    real_fcntl = dlsym(RTLD_NEXT, "fcntl");
    real_close = dlsym(RTLD_NEXT, "close");
}

int socket(int domain, int type, int protocol) {
    if (!real_socket) init();
    int fd = real_socket(domain, type, protocol);
    dprintf(2, "TRACE socket(domain=%d type=0x%x proto=%d) = %d errno=%d\n",
            domain, type, protocol, fd, errno);
    return fd;
}

int bind(int fd, const struct sockaddr *addr, socklen_t len) {
    if (!real_bind) init();
    int rc = real_bind(fd, addr, len);
    if (addr->sa_family == AF_INET) {
        struct sockaddr_in *sa = (struct sockaddr_in *)addr;
        char buf[32];
        inet_ntop(AF_INET, &sa->sin_addr, buf, sizeof(buf));
        dprintf(2, "TRACE bind(fd=%d %s:%d) = %d errno=%d\n",
                fd, buf, ntohs(sa->sin_port), rc, errno);
    } else {
        dprintf(2, "TRACE bind(fd=%d family=%d) = %d errno=%d\n",
                fd, addr->sa_family, rc, errno);
    }
    return rc;
}

int connect(int fd, const struct sockaddr *addr, socklen_t len) {
    if (!real_connect) init();
    int rc = real_connect(fd, addr, len);
    if (addr->sa_family == AF_INET) {
        struct sockaddr_in *sa = (struct sockaddr_in *)addr;
        char buf[32];
        inet_ntop(AF_INET, &sa->sin_addr, buf, sizeof(buf));
        dprintf(2, "TRACE connect(fd=%d %s:%d) = %d errno=%d\n",
                fd, buf, ntohs(sa->sin_port), rc, errno);
    } else {
        dprintf(2, "TRACE connect(fd=%d family=%d) = %d errno=%d\n",
                fd, addr->sa_family, rc, errno);
    }
    return rc;
}

int setsockopt(int fd, int level, int optname, const void *optval, socklen_t optlen) {
    if (!real_setsockopt) init();
    int rc = real_setsockopt(fd, level, optname, optval, optlen);
    dprintf(2, "TRACE setsockopt(fd=%d level=%d opt=%d len=%d) = %d errno=%d\n",
            fd, level, optname, optlen, rc, errno);
    return rc;
}

int getsockopt(int fd, int level, int optname, void *optval, socklen_t *optlen) {
    if (!real_getsockopt) init();
    int rc = real_getsockopt(fd, level, optname, optval, optlen);
    int val = (optval && optlen && *optlen >= 4) ? *(int *)optval : -1;
    dprintf(2, "TRACE getsockopt(fd=%d level=%d opt=%d) = %d val=%d errno=%d\n",
            fd, level, optname, rc, val, errno);
    return rc;
}

int getsockname(int fd, struct sockaddr *addr, socklen_t *addrlen) {
    if (!real_getsockname) init();
    int rc = real_getsockname(fd, addr, addrlen);
    dprintf(2, "TRACE getsockname(fd=%d) = %d errno=%d\n", fd, rc, errno);
    return rc;
}

ssize_t sendto(int fd, const void *buf, size_t len, int flags,
               const struct sockaddr *addr, socklen_t addrlen) {
    if (!real_sendto) init();
    ssize_t rc = real_sendto(fd, buf, len, flags, addr, addrlen);
    dprintf(2, "TRACE sendto(fd=%d len=%zu flags=0x%x) = %zd errno=%d\n",
            fd, len, flags, rc, errno);
    return rc;
}

ssize_t recvfrom(int fd, void *buf, size_t len, int flags,
                 struct sockaddr *addr, socklen_t *addrlen) {
    if (!real_recvfrom) init();
    ssize_t rc = real_recvfrom(fd, buf, len, flags, addr, addrlen);
    dprintf(2, "TRACE recvfrom(fd=%d len=%zu) = %zd errno=%d\n",
            fd, len, rc, errno);
    return rc;
}
