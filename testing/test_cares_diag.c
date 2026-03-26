// Direct c-ares diagnostic: tests each initialization step individually.
// Compiled in Alpine Docker against libcares.
// Build: gcc -o test-cares-diag test_cares_diag.c -lcares
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <unistd.h>
#include <signal.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <sys/select.h>
#include <poll.h>
#include <pthread.h>
#include <ares.h>

static void msg(const char *s) { write(1, s, strlen(s)); }

// ── Test 1: Can we create AF_INET6 socket? ──────────────────────────
static void test_ipv6_socket(void) {
    int fd = socket(AF_INET6, SOCK_DGRAM, 0);
    if (fd >= 0) {
        printf("DIAG ipv6_socket: fd=%d (supported)\n", fd);
        close(fd);
    } else {
        printf("DIAG ipv6_socket: errno=%d (%s) — IPv6 not supported\n",
               errno, strerror(errno));
    }
}

// ── Test 2: Can we create a thread? ─────────────────────────────────
static void *thread_func(void *arg) {
    *(int *)arg = 42;
    return NULL;
}

static void test_pthread(void) {
    int result = 0;
    pthread_t th;
    int rc = pthread_create(&th, NULL, thread_func, &result);
    if (rc != 0) {
        printf("DIAG pthread: create failed rc=%d\n", rc);
        return;
    }
    pthread_join(th, NULL);
    printf("DIAG pthread: OK (result=%d)\n", result);
}

// ── Test 3: ares_library_init ───────────────────────────────────────
static void test_ares_init(void) {
    int rc = ares_library_init(ARES_LIB_INIT_ALL);
    printf("DIAG ares_library_init: rc=%d (%s)\n", rc, ares_strerror(rc));
    if (rc != ARES_SUCCESS) return;

    // Try channel init with different options
    ares_channel_t *channel = NULL;

    // Option A: default init
    printf("DIAG ares_init: trying default...\n");
    fflush(stdout);
    rc = ares_init(&channel);
    printf("DIAG ares_init: rc=%d (%s)\n", rc, ares_strerror(rc));
    fflush(stdout);
    if (rc == ARES_SUCCESS) {
        ares_destroy(channel);
        channel = NULL;
    }

    // Option B: init with explicit options (no event thread)
    printf("DIAG ares_init_options: trying with explicit flags...\n");
    fflush(stdout);
    struct ares_options opts;
    memset(&opts, 0, sizeof(opts));
    opts.tries = 2;
    opts.timeout = 3000;
    // Don't use event thread — use traditional select loop
    rc = ares_init_options(&channel, &opts,
                           ARES_OPT_TRIES | ARES_OPT_TIMEOUTMS);
    printf("DIAG ares_init_options: rc=%d (%s)\n", rc, ares_strerror(rc));
    fflush(stdout);

    if (rc != ARES_SUCCESS) {
        ares_library_cleanup();
        return;
    }

    // Try resolution
    printf("DIAG ares: starting DNS query for example.com...\n");
    fflush(stdout);

    // Use the simpler synchronous-style approach
    volatile int done = 0;
    volatile int resolve_status = -1;
    volatile char resolved_ip[64] = {0};

    struct resolve_ctx {
        volatile int *done;
        volatile int *status;
        volatile char *ip;
    };
    struct resolve_ctx ctx = { &done, &resolve_status, resolved_ip };

    // We need a callback for ares_getaddrinfo
    // Actually, let's use the older ares_gethostbyname for simplicity
    // (though deprecated, still works)

    // Actually let me just manually send a DNS query via UDP to bypass c-ares entirely
    // and test the raw socket path
    ares_destroy(channel);
    ares_library_cleanup();
}

// ── Test 4: Manual UDP DNS query ────────────────────────────────────
static void test_manual_dns(void) {
    printf("DIAG manual_dns: creating UDP socket to 10.0.2.3:53...\n");
    fflush(stdout);

    int fd = socket(AF_INET, SOCK_DGRAM, 0);
    if (fd < 0) {
        printf("DIAG manual_dns: socket failed errno=%d\n", errno);
        return;
    }

    struct sockaddr_in server = {0};
    server.sin_family = AF_INET;
    server.sin_port = htons(53);
    inet_pton(AF_INET, "10.0.2.3", &server.sin_addr);

    // Connect (sets default destination for send)
    int rc = connect(fd, (struct sockaddr *)&server, sizeof(server));
    printf("DIAG manual_dns: connect rc=%d errno=%d\n", rc, errno);
    fflush(stdout);

    // Build DNS query for example.com A record
    // Header: ID=0x1234, QR=0, OPCODE=0, RD=1, QDCOUNT=1
    unsigned char query[] = {
        0x12, 0x34,  // ID
        0x01, 0x00,  // Flags: RD=1
        0x00, 0x01,  // QDCOUNT=1
        0x00, 0x00,  // ANCOUNT=0
        0x00, 0x00,  // NSCOUNT=0
        0x00, 0x00,  // ARCOUNT=0
        // QNAME: example.com
        7, 'e','x','a','m','p','l','e',
        3, 'c','o','m',
        0,           // root label
        0x00, 0x01,  // QTYPE=A
        0x00, 0x01,  // QCLASS=IN
    };

    ssize_t sent = send(fd, query, sizeof(query), 0);
    printf("DIAG manual_dns: send rc=%zd errno=%d\n", sent, errno);
    fflush(stdout);

    // Wait for response
    struct timeval tv = { .tv_sec = 5 };
    fd_set rfds;
    FD_ZERO(&rfds);
    FD_SET(fd, &rfds);
    rc = select(fd + 1, &rfds, NULL, NULL, &tv);
    printf("DIAG manual_dns: select rc=%d\n", rc);
    fflush(stdout);

    if (rc > 0) {
        unsigned char resp[512];
        struct sockaddr_in from;
        socklen_t fromlen = sizeof(from);
        ssize_t n = recvfrom(fd, resp, sizeof(resp), 0,
                             (struct sockaddr *)&from, &fromlen);
        printf("DIAG manual_dns: recvfrom rc=%zd\n", n);

        if (n >= 12) {
            int rcode = resp[3] & 0x0f;
            int ancount = (resp[6] << 8) | resp[7];
            printf("DIAG manual_dns: rcode=%d ancount=%d\n", rcode, ancount);

            // Parse answer to get IP
            if (ancount > 0 && n > 30) {
                // Skip question section: find end of QNAME
                int pos = 12;
                while (pos < n && resp[pos] != 0) pos += resp[pos] + 1;
                pos += 5; // skip null + QTYPE + QCLASS

                // Read first answer
                if (pos + 12 <= n) {
                    // Skip name (possibly compressed)
                    if ((resp[pos] & 0xc0) == 0xc0) pos += 2;
                    else { while (pos < n && resp[pos] != 0) pos += resp[pos] + 1; pos++; }

                    int atype = (resp[pos] << 8) | resp[pos+1];
                    int rdlen = (resp[pos+8] << 8) | resp[pos+9];
                    pos += 10;
                    if (atype == 1 && rdlen == 4 && pos + 4 <= n) {
                        printf("DIAG manual_dns: resolved %d.%d.%d.%d\n",
                               resp[pos], resp[pos+1], resp[pos+2], resp[pos+3]);
                    }
                }
            }
        }
    }
    close(fd);
}

// ── Test 5: c-ares with manual event loop (no threading) ────────────

static volatile int cares_done = 0;
static char cares_result[128] = {0};

static void cares_callback(void *arg, int status, int timeouts,
                           struct ares_addrinfo *result) {
    (void)arg;
    printf("DIAG cares_cb: status=%d timeouts=%d (%s)\n", status, timeouts, ares_strerror(status));
    fflush(stdout);
    if (status == ARES_SUCCESS && result) {
        struct ares_addrinfo_node *node = result->nodes;
        if (node && node->ai_family == AF_INET) {
            struct sockaddr_in *sa = (struct sockaddr_in *)node->ai_addr;
            inet_ntop(AF_INET, &sa->sin_addr, cares_result, sizeof(cares_result));
        }
        ares_freeaddrinfo(result);
    } else {
        snprintf(cares_result, sizeof(cares_result), "FAIL(code=%d): %s", status, ares_strerror(status));
    }
    cares_done = 1;
}

// ── Intercepting c-ares socket functions ─────────────────────────
static ares_socket_t my_asocket(int domain, int type, int protocol, void *ud) {
    (void)ud;
    // Mimic c-ares default: socket WITHOUT SOCK_NONBLOCK (c-ares sets it later)
    int fd = socket(domain, type, protocol);
    printf("DIAG cares_sock: socket(domain=%d type=%d proto=%d) = %d errno=%d\n",
           domain, type, protocol, fd, errno);
    fflush(stdout);
    return fd < 0 ? ARES_SOCKET_BAD : fd;
}
static int my_aclose(ares_socket_t fd, void *ud) {
    (void)ud;
    printf("DIAG cares_sock: close(%d)\n", (int)fd);
    fflush(stdout);
    return close(fd);
}
static int my_asetsockopt(ares_socket_t fd, ares_socket_opt_t opt,
                          const void *optval, ares_socklen_t optlen, void *ud) {
    (void)ud; (void)optval; (void)optlen;
    printf("DIAG cares_sock: setsockopt(fd=%d opt=%d) = 0\n", (int)fd, opt);
    fflush(stdout);
    return 0; // always succeed
}
static int my_aconnect(ares_socket_t fd, const struct sockaddr *addr,
                       ares_socklen_t len, unsigned int flags, void *ud) {
    (void)ud;
    char ipbuf[64] = "?";
    int port = 0;
    if (addr->sa_family == AF_INET) {
        struct sockaddr_in *sa = (struct sockaddr_in *)addr;
        inet_ntop(AF_INET, &sa->sin_addr, ipbuf, sizeof(ipbuf));
        port = ntohs(sa->sin_port);
    } else if (addr->sa_family == AF_INET6) {
        snprintf(ipbuf, sizeof(ipbuf), "AF_INET6");
    }
    int rc = connect(fd, addr, len);
    printf("DIAG cares_sock: connect(fd=%d %s:%d flags=%u) = %d errno=%d\n",
           (int)fd, ipbuf, port, flags, rc, errno);
    fflush(stdout);
    return rc;
}
static ares_ssize_t my_arecvfrom(ares_socket_t fd, void *buf, size_t len,
                                 int flags, struct sockaddr *addr,
                                 ares_socklen_t *alen, void *ud) {
    (void)ud;
    ssize_t n = recvfrom(fd, buf, len, flags, addr, alen);
    printf("DIAG cares_sock: recvfrom(fd=%d len=%zu) = %zd errno=%d\n",
           (int)fd, len, n, errno);
    fflush(stdout);
    return n;
}
static ares_ssize_t my_asendto(ares_socket_t fd, const void *buf, size_t len,
                               int flags, const struct sockaddr *addr,
                               ares_socklen_t alen, void *ud) {
    (void)ud;
    ssize_t n = sendto(fd, buf, len, flags, addr, alen);
    printf("DIAG cares_sock: sendto(fd=%d len=%zu flags=%d) = %zd errno=%d\n",
           (int)fd, len, flags, n, errno);
    fflush(stdout);
    return n;
}

static struct ares_socket_functions_ex my_sock_funcs = {
    .version = 1,
    .flags = 0,
    .asocket = my_asocket,
    .aclose = my_aclose,
    .asetsockopt = my_asetsockopt,
    .aconnect = my_aconnect,
    .arecvfrom = my_arecvfrom,
    .asendto = my_asendto,
};

static void test_cares_manual(void) {
    printf("DIAG cares_manual: init...\n");
    fflush(stdout);

    int rc = ares_library_init(ARES_LIB_INIT_ALL);
    if (rc != ARES_SUCCESS) {
        printf("DIAG cares_manual: library_init failed: %s\n", ares_strerror(rc));
        return;
    }

    ares_channel_t *channel = NULL;
    struct ares_options opts;
    memset(&opts, 0, sizeof(opts));
    opts.tries = 2;
    opts.timeout = 3000;
    opts.flags = ARES_FLAG_NOCHECKRESP;

    // Use poll-based event system (no internal event thread)
    opts.evsys = ARES_EVSYS_POLL;
    rc = ares_init_options(&channel, &opts,
                           ARES_OPT_TRIES | ARES_OPT_TIMEOUTMS | ARES_OPT_FLAGS
                           | ARES_OPT_EVENT_THREAD);
    printf("DIAG cares_manual: init_options rc=%d (%s)\n", rc, ares_strerror(rc));
    fflush(stdout);

    if (rc != ARES_SUCCESS) {
        ares_library_cleanup();
        return;
    }

    // Register socket function interceptors
    ares_set_socket_functions_ex(channel, &my_sock_funcs, NULL);
    printf("DIAG cares_manual: socket functions registered\n");
    fflush(stdout);

    // Check what servers were parsed from resolv.conf
    {
        struct ares_addr_port_node *servers = NULL;
        int nrc = ares_get_servers_ports(channel, &servers);
        printf("DIAG cares_manual: get_servers rc=%d\n", nrc);
        for (struct ares_addr_port_node *s = servers; s; s = s->next) {
            char addr[64];
            if (s->family == AF_INET)
                inet_ntop(AF_INET, &s->addr.addr4, addr, sizeof(addr));
            else
                snprintf(addr, sizeof(addr), "AF_%d", s->family);
            printf("DIAG cares_manual: server=%s udp=%d tcp=%d\n",
                   addr, s->udp_port, s->tcp_port);
        }
        if (servers) ares_free_data(servers);
        fflush(stdout);
    }

    // Set servers using CSV string (modern API)
    rc = ares_set_servers_ports_csv(channel, "10.0.2.3");
    printf("DIAG cares_manual: set_servers_csv rc=%d (%s)\n", rc, ares_strerror(rc));
    fflush(stdout);

    // Verify servers after setting
    {
        struct ares_addr_port_node *servers = NULL;
        ares_get_servers_ports(channel, &servers);
        int count = 0;
        for (struct ares_addr_port_node *s = servers; s; s = s->next) count++;
        printf("DIAG cares_manual: server_count after set=%d\n", count);
        if (servers) ares_free_data(servers);
        fflush(stdout);
    }

    // Test: getsockopt(SO_ERROR) on a fresh connected UDP socket
    {
        int tfd = socket(AF_INET, SOCK_DGRAM | SOCK_NONBLOCK | SOCK_CLOEXEC, 0);
        struct sockaddr_in sa = {0};
        sa.sin_family = AF_INET;
        sa.sin_port = htons(53);
        inet_pton(AF_INET, "10.0.2.3", &sa.sin_addr);
        connect(tfd, (struct sockaddr *)&sa, sizeof(sa));
        int so_err = -1;
        socklen_t so_len = sizeof(so_err);
        getsockopt(tfd, SOL_SOCKET, 4/*SO_ERROR*/, &so_err, &so_len);
        printf("DIAG cares_manual: fresh UDP SO_ERROR=%d (want 0)\n", so_err);
        fflush(stdout);
        close(tfd);
    }

    // Start async query using ares_getaddrinfo
    struct ares_addrinfo_hints hints;
    memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_INET;
    hints.ai_flags = ARES_AI_CANONNAME;
    cares_done = 0;
    cares_result[0] = 0;

    ares_getaddrinfo(channel, "example.com", NULL, &hints, cares_callback, NULL);
    printf("DIAG cares_manual: query submitted, done=%d\n", cares_done);
    fflush(stdout);

    // Try kicking the resolver explicitly
    if (!cares_done) {
        printf("DIAG cares_manual: calling ares_process_fd(BAD,BAD) to kick...\n");
        fflush(stdout);
        ares_process_fd(channel, ARES_SOCKET_BAD, ARES_SOCKET_BAD);
        printf("DIAG cares_manual: after kick, done=%d\n", cares_done);
        fflush(stdout);
    }

    printf("DIAG cares_manual: entering event loop...\n");
    fflush(stdout);

    // Manual select loop (no threading)
    for (int iter = 0; iter < 50 && !cares_done; iter++) {
        fd_set read_fds, write_fds;
        int nfds;
        struct timeval tv, *tvp;
        FD_ZERO(&read_fds);
        FD_ZERO(&write_fds);
        nfds = ares_fds(channel, &read_fds, &write_fds);
        if (nfds == 0) {
            printf("DIAG cares_manual: no fds (iter=%d)\n", iter);
            break;
        }
        tvp = ares_timeout(channel, NULL, &tv);
        printf("DIAG cares_manual: select nfds=%d timeout=%ld.%06ld (iter=%d)\n",
               nfds, (long)(tvp ? tvp->tv_sec : -1),
               (long)(tvp ? tvp->tv_usec : 0), iter);
        fflush(stdout);
        rc = select(nfds, &read_fds, &write_fds, NULL, tvp);
        printf("DIAG cares_manual: select returned %d errno=%d\n", rc, errno);
        fflush(stdout);
        ares_process(channel, &read_fds, &write_fds);
    }

    printf("DIAG cares_manual: done=%d result='%s'\n", cares_done, cares_result);
    fflush(stdout);

    ares_destroy(channel);
    ares_library_cleanup();
}

// ── Test 6: UDP DNS from a separate thread ──────────────────────────

static void *thread_dns_func(void *arg) {
    (void)arg;
    printf("DIAG thread_dns: thread started\n");
    fflush(stdout);

    int fd = socket(AF_INET, SOCK_DGRAM | SOCK_NONBLOCK | SOCK_CLOEXEC, 0);
    printf("DIAG thread_dns: socket fd=%d errno=%d\n", fd, errno);
    fflush(stdout);
    if (fd < 0) return (void *)(long)-1;

    struct sockaddr_in server = {0};
    server.sin_family = AF_INET;
    server.sin_port = htons(53);
    inet_pton(AF_INET, "10.0.2.3", &server.sin_addr);

    int rc = connect(fd, (struct sockaddr *)&server, sizeof(server));
    printf("DIAG thread_dns: connect rc=%d errno=%d\n", rc, errno);
    fflush(stdout);

    unsigned char query[] = {
        0xAB, 0xCD, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00,
        7, 'e','x','a','m','p','l','e', 3, 'c','o','m', 0,
        0x00, 0x01, 0x00, 0x01,
    };
    ssize_t sent = send(fd, query, sizeof(query), MSG_NOSIGNAL);
    printf("DIAG thread_dns: send rc=%zd errno=%d\n", sent, errno);
    fflush(stdout);

    // Use poll to wait
    struct pollfd pfd = { .fd = fd, .events = POLLIN };
    rc = poll(&pfd, 1, 5000);
    printf("DIAG thread_dns: poll rc=%d revents=0x%x\n", rc, pfd.revents);
    fflush(stdout);

    if (rc > 0 && (pfd.revents & POLLIN)) {
        unsigned char resp[512];
        ssize_t n = recv(fd, resp, sizeof(resp), 0);
        printf("DIAG thread_dns: recv rc=%zd\n", n);
        if (n >= 12) {
            int rcode = resp[3] & 0x0f;
            int ancount = (resp[6] << 8) | resp[7];
            printf("DIAG thread_dns: rcode=%d ancount=%d\n", rcode, ancount);
            // Parse IP from first A record
            if (ancount > 0) {
                int pos = 12;
                while (pos < n && resp[pos] != 0) pos += resp[pos] + 1;
                pos += 5;
                if (pos + 12 <= n) {
                    if ((resp[pos] & 0xc0) == 0xc0) pos += 2;
                    else { while (pos < n && resp[pos] != 0) pos += resp[pos] + 1; pos++; }
                    int rdlen = (resp[pos+8] << 8) | resp[pos+9];
                    pos += 10;
                    if (rdlen == 4 && pos + 4 <= n)
                        printf("DIAG thread_dns: resolved %d.%d.%d.%d\n",
                               resp[pos], resp[pos+1], resp[pos+2], resp[pos+3]);
                }
            }
        }
        fflush(stdout);
    }
    close(fd);
    return NULL;
}

static void test_thread_dns(void) {
    printf("DIAG thread_dns: starting thread for UDP DNS test...\n");
    fflush(stdout);
    pthread_t th;
    int rc = pthread_create(&th, NULL, thread_dns_func, NULL);
    if (rc != 0) {
        printf("DIAG thread_dns: pthread_create failed rc=%d\n", rc);
        return;
    }
    void *retval;
    pthread_join(th, &retval);
    printf("DIAG thread_dns: thread finished retval=%ld\n", (long)retval);
    fflush(stdout);
}

// ── Test 7: c-ares WITHOUT interceptors (default socket path) ───────
static void test_cares_default(void) {
    printf("DIAG cares_default: init (NO interceptors)...\n");
    fflush(stdout);

    int rc = ares_library_init(ARES_LIB_INIT_ALL);
    if (rc != ARES_SUCCESS) {
        printf("DIAG cares_default: library_init failed: %s\n", ares_strerror(rc));
        return;
    }

    ares_channel_t *channel = NULL;
    struct ares_options opts;
    memset(&opts, 0, sizeof(opts));
    opts.tries = 2;
    opts.timeout = 3000;
    opts.flags = ARES_FLAG_NOCHECKRESP;
    opts.evsys = ARES_EVSYS_POLL;

    rc = ares_init_options(&channel, &opts,
                           ARES_OPT_TRIES | ARES_OPT_TIMEOUTMS | ARES_OPT_FLAGS
                           | ARES_OPT_EVENT_THREAD);
    printf("DIAG cares_default: init_options rc=%d (%s)\n", rc, ares_strerror(rc));
    fflush(stdout);
    if (rc != ARES_SUCCESS) { ares_library_cleanup(); return; }

    // Set servers
    ares_set_servers_ports_csv(channel, "10.0.2.3");

    // NO ares_set_socket_functions_ex — use c-ares default socket path

    struct ares_addrinfo_hints hints;
    memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_INET;
    cares_done = 0;
    cares_result[0] = 0;

    printf("DIAG cares_default: submitting query...\n");
    fflush(stdout);
    ares_getaddrinfo(channel, "example.com", NULL, &hints, cares_callback, NULL);
    printf("DIAG cares_default: query submitted, done=%d\n", cares_done);
    fflush(stdout);

    // Try manual event loop
    for (int iter = 0; iter < 20 && !cares_done; iter++) {
        fd_set read_fds, write_fds;
        struct timeval tv, *tvp;
        FD_ZERO(&read_fds);
        FD_ZERO(&write_fds);
        int nfds = ares_fds(channel, &read_fds, &write_fds);
        if (nfds == 0) {
            printf("DIAG cares_default: no fds (iter=%d)\n", iter);
            break;
        }
        tvp = ares_timeout(channel, NULL, &tv);
        select(nfds, &read_fds, &write_fds, NULL, tvp);
        ares_process(channel, &read_fds, &write_fds);
    }
    printf("DIAG cares_default: done=%d result='%s'\n", cares_done, cares_result);
    fflush(stdout);

    ares_destroy(channel);
    ares_library_cleanup();
}

int main(void) {
    printf("=== c-ares DNS Diagnostic ===\n");

    test_ipv6_socket();
    test_pthread();
    test_ares_init();
    test_manual_dns();
    test_thread_dns();

    // Run default path FIRST (with LD_PRELOAD tracing)
    test_cares_default();

    // Then run with interceptors
    test_cares_manual();

    printf("=== Done ===\n");
    return 0;
}
