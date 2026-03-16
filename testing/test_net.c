// Network diagnostic test for Kevlar.
// Tests each part of the TCP/UDP path independently.
#include <sys/socket.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <unistd.h>
#include <string.h>
#include <stdio.h>
#include <poll.h>
#include <errno.h>
#include <fcntl.h>

static void check(const char *msg, int ok) {
    if (ok) printf("PASS %s\n", msg);
    else    printf("FAIL %s (errno=%d)\n", msg, errno);
}

// Test 1: UDP send+recv to QEMU's DNS server (10.0.2.3:53)
static void test_udp_dns(void) {
    printf("=== UDP DNS test ===\n");

    int fd = socket(AF_INET, SOCK_DGRAM, 0);
    check("udp_socket", fd >= 0);
    if (fd < 0) return;

    struct sockaddr_in dns = {
        .sin_family = AF_INET,
        .sin_port = htons(53),
    };
    inet_aton("10.0.2.3", &dns.sin_addr);

    // Bind to a specific port so we know what to expect.
    struct sockaddr_in local = {
        .sin_family = AF_INET,
        .sin_port = htons(44444),
        .sin_addr.s_addr = INADDR_ANY,
    };
    int r = bind(fd, (struct sockaddr *)&local, sizeof(local));
    check("udp_bind", r == 0);

    // Also connect (sets peer address for send).
    r = connect(fd, (struct sockaddr *)&dns, sizeof(dns));
    check("udp_connect", r == 0);

    // Minimal DNS query for "example.com" (type A, class IN)
    unsigned char query[] = {
        0x12, 0x34,  // ID
        0x01, 0x00,  // Flags: standard query
        0x00, 0x01,  // QDCOUNT: 1
        0x00, 0x00,  // ANCOUNT: 0
        0x00, 0x00,  // NSCOUNT: 0
        0x00, 0x00,  // ARCOUNT: 0
        // QNAME: example.com
        7, 'e','x','a','m','p','l','e',
        3, 'c','o','m',
        0,           // root label
        0x00, 0x01,  // QTYPE: A
        0x00, 0x01,  // QCLASS: IN
    };

    // Use sendto with explicit address (not relying on connect peer).
    ssize_t sent = sendto(fd, query, sizeof(query), 0,
                          (struct sockaddr *)&dns, sizeof(dns));
    printf("  udp sendto: %zd bytes\n", sent);
    check("udp_send", sent == sizeof(query));

    // Poll for response
    struct pollfd pfd = { .fd = fd, .events = POLLIN };
    printf("  polling for DNS response (5 sec timeout)...\n");
    r = poll(&pfd, 1, 5000);
    printf("  poll returned %d, revents=0x%x\n", r, pfd.revents);
    check("udp_poll", r > 0);

    if (r > 0) {
        unsigned char resp[512];
        ssize_t n = recv(fd, resp, sizeof(resp), 0);
        printf("  recv: %zd bytes\n", n);
        check("udp_recv", n > 0);
        if (n >= 4) {
            printf("  DNS response ID=0x%02x%02x, flags=0x%02x%02x\n",
                   resp[0], resp[1], resp[2], resp[3]);
        }
    }

    close(fd);
}

// Test 2: TCP connect to QEMU gateway (10.0.2.2:80 — might not have HTTP server)
static void test_tcp_connect(void) {
    printf("=== TCP connect test ===\n");

    int fd = socket(AF_INET, SOCK_STREAM, 0);
    check("tcp_socket", fd >= 0);
    if (fd < 0) return;

    // Try connecting to QEMU's built-in HTTP redirect (port 80)
    struct sockaddr_in addr = {
        .sin_family = AF_INET,
        .sin_port = htons(80),
    };
    // Use a known reachable IP — the QEMU gateway
    inet_aton("10.0.2.2", &addr.sin_addr);

    printf("  connecting to 10.0.2.2:80...\n");
    int r = connect(fd, (struct sockaddr *)&addr, sizeof(addr));
    printf("  connect returned %d (errno=%d)\n", r, errno);
    // Gateway might not have port 80 open — that's OK
    if (r == 0) {
        check("tcp_connect", 1);

        // Send a simple HTTP GET
        const char *req = "GET / HTTP/1.0\r\nHost: 10.0.2.2\r\n\r\n";
        ssize_t sent = write(fd, req, strlen(req));
        printf("  sent %zd bytes\n", sent);

        // Poll for response
        struct pollfd pfd = { .fd = fd, .events = POLLIN };
        r = poll(&pfd, 1, 5000);
        printf("  poll returned %d, revents=0x%x\n", r, pfd.revents);

        if (r > 0) {
            char buf[256];
            ssize_t n = read(fd, buf, sizeof(buf) - 1);
            printf("  read %zd bytes\n", n);
            if (n > 0) {
                buf[n] = 0;
                printf("  first line: %.60s\n", buf);
                check("tcp_read", n > 0);
            }
        } else {
            printf("  FAIL: no response from server\n");
        }
    } else {
        printf("  connect failed (expected if no HTTP server on gateway)\n");
    }

    close(fd);
}

// Test 3: TCP connect to Alpine CDN (151.101.126.132:80) — the real target
static void test_tcp_cdn(void) {
    printf("=== TCP CDN test ===\n");

    int fd = socket(AF_INET, SOCK_STREAM, 0);
    check("tcp_cdn_socket", fd >= 0);
    if (fd < 0) return;

    struct sockaddr_in addr = {
        .sin_family = AF_INET,
        .sin_port = htons(80),
    };
    inet_aton("151.101.126.132", &addr.sin_addr);

    printf("  connecting to 151.101.126.132:80 (Alpine CDN)...\n");
    int r = connect(fd, (struct sockaddr *)&addr, sizeof(addr));
    printf("  connect returned %d (errno=%d)\n", r, errno);
    check("tcp_cdn_connect", r == 0);
    if (r != 0) { close(fd); return; }

    // Send HTTP GET
    const char *req =
        "GET /alpine/v3.21/main/x86_64/APKINDEX.tar.gz HTTP/1.0\r\n"
        "Host: dl-cdn.alpinelinux.org\r\n"
        "\r\n";
    ssize_t sent = write(fd, req, strlen(req));
    printf("  sent %zd bytes\n", sent);
    check("tcp_cdn_send", sent > 0);

    // Read response in chunks
    printf("  reading response...\n");
    size_t total = 0;
    char buf[4096];
    int got_header = 0;
    for (int i = 0; i < 200; i++) {
        struct pollfd pfd = { .fd = fd, .events = POLLIN };
        r = poll(&pfd, 1, 3000);
        if (r <= 0) {
            printf("  poll timeout after %zu bytes (round %d)\n", total, i);
            break;
        }
        ssize_t n = read(fd, buf, sizeof(buf));
        if (n <= 0) {
            printf("  read returned %zd (EOF or error) after %zu bytes\n", n, total);
            break;
        }
        total += n;
        if (!got_header && total > 20) {
            buf[n < 80 ? n : 80] = 0;
            printf("  first bytes: %.60s\n", buf);
            got_header = 1;
        }
    }
    printf("  total received: %zu bytes\n", total);
    check("tcp_cdn_download", total > 1000);

    close(fd);
}

int main(void) {
    printf("TEST_START net_diag\n");

    // Wait for DHCP
    sleep(2);

    test_udp_dns();
    test_tcp_connect();
    test_tcp_cdn();

    printf("TEST_END\n");
    return 0;
}
