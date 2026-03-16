// Alpine Linux integration test for Kevlar.
// Tests 7 layers bottom-up: ext2 → write → chroot → apk DB → DNS → HTTP → apk update.
// Each layer depends on the previous — if chroot fails, we skip apk tests.
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <unistd.h>
#include <string.h>
#include <stdio.h>
#include <stdlib.h>
#include <fcntl.h>
#include <poll.h>
#include <errno.h>
#include <signal.h>
#include <dirent.h>

// ─── Test Accounting ─────────────────────────────────────────────────────────

static int g_pass, g_fail, g_skip;

static int pass(const char *name) {
    printf("PASS %s\n", name);
    g_pass++;
    return 1;
}

static int fail(const char *name, const char *detail) {
    if (detail)
        printf("FAIL %s (%s)\n", name, detail);
    else
        printf("FAIL %s\n", name);
    g_fail++;
    return 0;
}

static void skip(const char *name, const char *reason) {
    printf("SKIP %s (%s)\n", name, reason);
    g_skip++;
}

static void skip_layer(int layer, const char *name, const char *reason,
                       const char **tests, int count) {
    printf("LAYER %d: %s [SKIPPED: %s]\n", layer, name, reason);
    for (int i = 0; i < count; i++)
        skip(tests[i], reason);
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

// Fork, chroot, exec, capture stdout. Returns exit status or -1.
static int chroot_exec_capture(const char *rootdir, char *const argv[],
                               char *out, int outsz, int timeout_ms) {
    int pipefd[2];
    if (pipe(pipefd) < 0) return -1;

    pid_t pid = fork();
    if (pid < 0) { close(pipefd[0]); close(pipefd[1]); return -1; }

    if (pid == 0) {
        // Child: chroot + exec
        close(pipefd[0]);
        dup2(pipefd[1], STDOUT_FILENO);
        dup2(pipefd[1], STDERR_FILENO);
        close(pipefd[1]);
        write(STDERR_FILENO, "C:chroot\n", 9);
        if (chroot(rootdir) < 0) _exit(126);
        write(STDERR_FILENO, "C:chdir\n", 8);
        if (chdir("/") < 0) _exit(126);
        write(STDERR_FILENO, "C:exec\n", 7);
        execve(argv[0], argv, NULL);
        // If execve returns, print errno
        char ebuf[32];
        int len = snprintf(ebuf, sizeof(ebuf), "C:exec_fail=%d\n", errno);
        write(STDERR_FILENO, ebuf, len);
        _exit(127);
    }

    // Parent: read output with timeout
    close(pipefd[1]);
    int pos = 0;
    struct pollfd pfd = { .fd = pipefd[0], .events = POLLIN };
    while (pos < outsz - 1) {
        int r = poll(&pfd, 1, timeout_ms);
        if (r <= 0) break;
        ssize_t n = read(pipefd[0], out + pos, outsz - 1 - pos);
        if (n <= 0) break;
        pos += n;
    }
    out[pos] = '\0';
    close(pipefd[0]);

    // Wait for child with timeout (poll on nothing, then WNOHANG).
    int status = 0;
    int waited = 0;
    for (int i = 0; i < timeout_ms / 100 + 1; i++) {
        pid_t w = waitpid(pid, &status, WNOHANG);
        if (w > 0) { waited = 1; break; }
        if (w < 0) break;
        usleep(100000); // 100ms
    }
    if (!waited) {
        kill(pid, SIGKILL);
        waitpid(pid, &status, 0);
        return -2; // timeout
    }
    if (WIFEXITED(status)) return WEXITSTATUS(status);
    if (WIFSIGNALED(status)) return 128 + WTERMSIG(status);
    return -1;
}

// Check if a file exists (follows symlinks).
static int file_exists(const char *path) {
    struct stat st;
    return stat(path, &st) == 0;
}

// Check if a path exists without following symlinks.
static int link_exists(const char *path) {
    struct stat st;
    return lstat(path, &st) == 0;
}

// ─── Layer 1: Foundation ─────────────────────────────────────────────────────

static int layer1_foundation(void) {
    printf("LAYER 1: foundation\n");
    int ok = 1;
    char detail[128];

    // Mount the Alpine ext2 disk image.
    mkdir("/mnt", 0755);
    int r = mount("/dev/vda", "/mnt", "ext2", 0, NULL);
    if (r == 0)
        pass("l1_mount_ext2");
    else {
        snprintf(detail, sizeof(detail), "mount failed errno=%d", errno);
        fail("l1_mount_ext2", detail);
        return 0;  // Can't continue without mount
    }

    // Verify Alpine files exist.
    if (file_exists("/mnt/bin/busybox"))
        pass("l1_busybox_exists");
    else { fail("l1_busybox_exists", "not found"); ok = 0; }

    if (file_exists("/mnt/lib/ld-musl-x86_64.so.1"))
        pass("l1_musl_ld_exists");
    else { fail("l1_musl_ld_exists", "not found"); ok = 0; }

    if (file_exists("/mnt/sbin/apk"))
        pass("l1_apk_exists");
    else { fail("l1_apk_exists", "not found"); ok = 0; }

    if (file_exists("/mnt/etc/apk/repositories"))
        pass("l1_repositories_exists");
    else { fail("l1_repositories_exists", "not found"); ok = 0; }

    // Read /mnt/etc/apk/arch, verify "x86_64".
    {
        int fd = open("/mnt/etc/apk/arch", O_RDONLY);
        if (fd >= 0) {
            char buf[32];
            ssize_t n = read(fd, buf, sizeof(buf) - 1);
            close(fd);
            if (n > 0) {
                buf[n] = '\0';
                if (strncmp(buf, "x86_64", 6) == 0)
                    pass("l1_arch_x86_64");
                else {
                    snprintf(detail, sizeof(detail), "got '%s'", buf);
                    fail("l1_arch_x86_64", detail);
                    ok = 0;
                }
            } else { fail("l1_arch_x86_64", "empty file"); ok = 0; }
        } else { fail("l1_arch_x86_64", "cannot open"); ok = 0; }
    }

    // Stat busybox: executable, size > 100KB.
    {
        struct stat st;
        if (stat("/mnt/bin/busybox", &st) == 0) {
            if ((st.st_mode & S_IXUSR) && st.st_size > 100 * 1024)
                pass("l1_busybox_stat");
            else {
                snprintf(detail, sizeof(detail), "mode=0%o size=%ld",
                         (unsigned)st.st_mode, (long)st.st_size);
                fail("l1_busybox_stat", detail);
                ok = 0;
            }
        } else { fail("l1_busybox_stat", "stat failed"); ok = 0; }
    }

    return ok;
}

// ─── Layer 2: ext2 Write ─────────────────────────────────────────────────────

static int layer2_ext2_write(void) {
    printf("LAYER 2: ext2_write\n");
    int ok = 1;
    char detail[128];

    // Cleanup from prior runs.
    unlink("/mnt/tmp/test_alpine_file");
    unlink("/mnt/tmp/test_alpine_renamed");
    unlink("/mnt/tmp/test_alpine_link");
    unlink("/mnt/tmp/test_alpine_large");
    rmdir("/mnt/tmp/test_alpine_dir");
    mkdir("/mnt/tmp", 0755);

    // Create, write, read.
    {
        int fd = open("/mnt/tmp/test_alpine_file", O_CREAT | O_WRONLY | O_TRUNC, 0644);
        if (fd >= 0) {
            const char *msg = "hello alpine\n";
            write(fd, msg, strlen(msg));
            close(fd);

            fd = open("/mnt/tmp/test_alpine_file", O_RDONLY);
            char buf[64];
            ssize_t n = read(fd, buf, sizeof(buf) - 1);
            close(fd);
            if (n > 0) { buf[n] = '\0'; }
            if (n > 0 && strcmp(buf, "hello alpine\n") == 0)
                pass("l2_create_write_read");
            else { fail("l2_create_write_read", "data mismatch"); ok = 0; }
        } else { fail("l2_create_write_read", "open failed"); ok = 0; }
    }

    // mkdir + rmdir.
    if (mkdir("/mnt/tmp/test_alpine_dir", 0755) == 0 &&
        file_exists("/mnt/tmp/test_alpine_dir") &&
        rmdir("/mnt/tmp/test_alpine_dir") == 0 &&
        !file_exists("/mnt/tmp/test_alpine_dir"))
        pass("l2_mkdir_rmdir");
    else { fail("l2_mkdir_rmdir", NULL); ok = 0; }

    // symlink — use lstat to check existence without following the link.
    if (symlink("test_alpine_file", "/mnt/tmp/test_alpine_link") == 0 &&
        link_exists("/mnt/tmp/test_alpine_link"))
        pass("l2_symlink");
    else {
        snprintf(detail, sizeof(detail), "symlink errno=%d", errno);
        fail("l2_symlink", detail);
        ok = 0;
    }
    unlink("/mnt/tmp/test_alpine_link");

    // rename.
    if (rename("/mnt/tmp/test_alpine_file", "/mnt/tmp/test_alpine_renamed") == 0 &&
        file_exists("/mnt/tmp/test_alpine_renamed") &&
        !file_exists("/mnt/tmp/test_alpine_file"))
        pass("l2_rename");
    else { fail("l2_rename", NULL); ok = 0; }
    unlink("/mnt/tmp/test_alpine_renamed");

    // Large file (8KB).
    {
        int fd = open("/mnt/tmp/test_alpine_large", O_CREAT | O_WRONLY | O_TRUNC, 0644);
        if (fd >= 0) {
            char buf[1024];
            memset(buf, 'A', sizeof(buf));
            int wrote = 0;
            for (int i = 0; i < 8; i++) {
                ssize_t n = write(fd, buf, sizeof(buf));
                if (n > 0) wrote += n;
            }
            close(fd);
            struct stat st;
            stat("/mnt/tmp/test_alpine_large", &st);
            if (wrote == 8192 && st.st_size == 8192)
                pass("l2_large_file");
            else {
                snprintf(detail, sizeof(detail), "wrote=%d size=%ld", wrote, (long)st.st_size);
                fail("l2_large_file", detail);
                ok = 0;
            }
            unlink("/mnt/tmp/test_alpine_large");
        } else { fail("l2_large_file", "open failed"); ok = 0; }
    }

    return ok;
}

// ─── Layer 3: chroot + Dynamic Linking ───────────────────────────────────────

static int layer3_chroot_dynlink(void) {
    printf("LAYER 3: chroot_dynlink\n");
    int ok = 1;
    char detail[128];

    // Mount /proc and /dev inside Alpine rootfs for chroot tests.
    mkdir("/mnt/proc", 0755);
    mkdir("/mnt/dev", 0755);
    mount("proc", "/mnt/proc", "proc", 0, NULL);
    mount("devtmpfs", "/mnt/dev", "devtmpfs", 0, NULL);

    // BusyBox --help.
    {
        char out[4096];
        char *argv[] = { "/bin/busybox", "--help", NULL };
        int rc = chroot_exec_capture("/mnt", argv, out, sizeof(out), 10000);
        if (rc >= 0 && strstr(out, "BusyBox"))
            pass("l3_busybox_help");
        else {
            snprintf(detail, sizeof(detail), "exit=%d out='%.60s'", rc, out);
            fail("l3_busybox_help", detail);
            ok = 0;
        }
    }

    // apk --version.
    {
        char out[4096];
        char *argv[] = { "/sbin/apk", "--version", NULL };
        int rc = chroot_exec_capture("/mnt", argv, out, sizeof(out), 10000);
        if ((rc == 0 || rc == 1) && strstr(out, "apk-tools"))
            pass("l3_apk_version");
        else {
            snprintf(detail, sizeof(detail), "exit=%d out='%.40s'", rc, out);
            fail("l3_apk_version", detail);
            ok = 0;
        }
    }

    return ok;
}

// ─── Layer 4: Alpine Package Database ────────────────────────────────────────

static int layer4_apk_database(void) {
    printf("LAYER 4: apk_database\n");
    int ok = 1;
    char detail[128];

    // apk info — list installed packages.
    {
        char out[8192];
        char *argv[] = { "/sbin/apk", "info", NULL };
        int rc = chroot_exec_capture("/mnt", argv, out, sizeof(out), 10000);
        if (rc != 0) {
            snprintf(detail, sizeof(detail), "exit=%d", rc);
            fail("l4_apk_info", detail);
            return 0;
        }

        // Should contain some base packages.
        int has_musl = strstr(out, "musl") != NULL;
        int has_busybox = strstr(out, "busybox") != NULL;
        int has_baselayout = strstr(out, "alpine-baselayout") != NULL;

        if (has_musl && has_busybox && has_baselayout)
            pass("l4_apk_info");
        else {
            snprintf(detail, sizeof(detail), "musl=%d bb=%d bl=%d",
                     has_musl, has_busybox, has_baselayout);
            fail("l4_apk_info", detail);
            ok = 0;
        }

        // Count packages: at least 10.
        int count = 0;
        for (char *p = out; *p; p++) {
            if (*p == '\n') count++;
        }
        if (count >= 10)
            pass("l4_package_count");
        else {
            snprintf(detail, sizeof(detail), "only %d packages", count);
            fail("l4_package_count", detail);
            ok = 0;
        }
    }

    return ok;
}

// ─── Layer 5: DNS Resolution ─────────────────────────────────────────────────

// Resolved IP stored for layer 6.
static struct in_addr g_resolved_ip;
static int g_dns_ok;

static int layer5_dns(void) {
    printf("LAYER 5: dns_resolution\n");
    int ok = 1;
    char detail[128];
    g_dns_ok = 0;

    int fd = socket(AF_INET, SOCK_DGRAM, 0);
    if (fd < 0) {
        snprintf(detail, sizeof(detail), "socket errno=%d", errno);
        fail("l5_udp_socket", detail);
        return 0;
    }
    pass("l5_udp_socket");

    struct sockaddr_in dns = {
        .sin_family = AF_INET,
        .sin_port = htons(53),
    };
    inet_aton("10.0.2.3", &dns.sin_addr);

    struct sockaddr_in local = {
        .sin_family = AF_INET,
        .sin_port = htons(0),
        .sin_addr.s_addr = INADDR_ANY,
    };
    if (bind(fd, (struct sockaddr *)&local, sizeof(local)) < 0) {
        snprintf(detail, sizeof(detail), "bind errno=%d", errno);
        fail("l5_dns_bind", detail);
        close(fd);
        return 0;
    }

    if (connect(fd, (struct sockaddr *)&dns, sizeof(dns)) < 0) {
        snprintf(detail, sizeof(detail), "connect errno=%d", errno);
        fail("l5_dns_connect", detail);
        close(fd);
        return 0;
    }

    // DNS query for dl-cdn.alpinelinux.org (type A).
    // Encoded labels: 6 dl-cdn 14 alpinelinux 3 org 0
    unsigned char query[] = {
        0xAB, 0xCD,  // ID
        0x01, 0x00,  // Flags: standard query, recursion desired
        0x00, 0x01,  // QDCOUNT: 1
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // AN/NS/AR = 0
        // QNAME: dl-cdn.alpinelinux.org
        6, 'd','l','-','c','d','n',
        14, 'a','l','p','i','n','e','l','i','n','u','x','.','o','r','g',
        0,
        0x00, 0x01,  // QTYPE: A
        0x00, 0x01,  // QCLASS: IN
    };
    // Fix: alpinelinux.org should be two labels: "alpinelinux" (11) and "org" (3)
    unsigned char query2[] = {
        0xAB, 0xCD,
        0x01, 0x00,
        0x00, 0x01,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        6, 'd','l','-','c','d','n',
        11, 'a','l','p','i','n','e','l','i','n','u','x',
        3, 'o','r','g',
        0,
        0x00, 0x01,
        0x00, 0x01,
    };

    ssize_t sent = send(fd, query2, sizeof(query2), 0);
    if (sent != sizeof(query2)) {
        snprintf(detail, sizeof(detail), "sent=%zd errno=%d", sent, errno);
        fail("l5_dns_send", detail);
        close(fd);
        return 0;
    }
    pass("l5_dns_send");

    // Poll with 5-second timeout.
    struct pollfd pfd = { .fd = fd, .events = POLLIN };
    int r = poll(&pfd, 1, 5000);
    if (r <= 0) {
        snprintf(detail, sizeof(detail), "poll=%d errno=%d", r, errno);
        fail("l5_dns_response", detail);
        close(fd);
        return 0;
    }

    unsigned char resp[512];
    ssize_t n = recv(fd, resp, sizeof(resp), 0);
    close(fd);

    if (n < 12) {
        snprintf(detail, sizeof(detail), "recv=%zd", n);
        fail("l5_dns_response", detail);
        return 0;
    }

    // Check ANCOUNT > 0.
    int ancount = (resp[6] << 8) | resp[7];
    if (ancount <= 0) {
        fail("l5_dns_response", "ANCOUNT=0");
        return 0;
    }
    pass("l5_dns_response");

    // Extract first A record IP from answer section.
    // Skip question section: find the end of QNAME + 4 bytes (QTYPE+QCLASS).
    int pos = 12;
    while (pos < n && resp[pos] != 0) {
        if ((resp[pos] & 0xc0) == 0xc0) { pos += 2; goto answers; }
        pos += 1 + resp[pos];
    }
    pos += 1 + 4;  // null label + QTYPE(2) + QCLASS(2)

answers:
    // Parse answer RRs looking for type A.
    for (int i = 0; i < ancount && pos + 12 <= n; i++) {
        // Skip NAME (may be compressed).
        if ((resp[pos] & 0xc0) == 0xc0) pos += 2;
        else { while (pos < n && resp[pos]) pos += 1 + resp[pos]; pos++; }
        if (pos + 10 > n) break;
        int rtype = (resp[pos] << 8) | resp[pos + 1];
        int rdlen = (resp[pos + 8] << 8) | resp[pos + 9];
        pos += 10;
        if (rtype == 1 && rdlen == 4 && pos + 4 <= n) {
            memcpy(&g_resolved_ip, resp + pos, 4);
            g_dns_ok = 1;
            char ipbuf[32];
            snprintf(ipbuf, sizeof(ipbuf), "%d.%d.%d.%d",
                     resp[pos], resp[pos+1], resp[pos+2], resp[pos+3]);
            printf("  resolved: %s\n", ipbuf);
            pass("l5_dns_resolved");
            return 1;
        }
        pos += rdlen;
    }

    fail("l5_dns_resolved", "no A record found");
    return 0;
}

// ─── Layer 6: TCP HTTP ───────────────────────────────────────────────────────

static int layer6_tcp_http(void) {
    printf("LAYER 6: tcp_http\n");
    int ok = 1;
    char detail[128];

    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) {
        snprintf(detail, sizeof(detail), "socket errno=%d", errno);
        fail("l6_tcp_socket", detail);
        return 0;
    }
    pass("l6_tcp_socket");

    struct sockaddr_in addr = {
        .sin_family = AF_INET,
        .sin_port = htons(80),
        .sin_addr = g_resolved_ip,
    };

    int r = connect(fd, (struct sockaddr *)&addr, sizeof(addr));
    if (r < 0) {
        snprintf(detail, sizeof(detail), "connect errno=%d", errno);
        fail("l6_tcp_connect", detail);
        close(fd);
        return 0;
    }
    pass("l6_tcp_connect");

    const char *req =
        "GET /alpine/v3.21/main/x86_64/APKINDEX.tar.gz HTTP/1.0\r\n"
        "Host: dl-cdn.alpinelinux.org\r\n"
        "\r\n";
    ssize_t sent = write(fd, req, strlen(req));
    if (sent <= 0) {
        snprintf(detail, sizeof(detail), "write=%zd errno=%d", sent, errno);
        fail("l6_http_send", detail);
        close(fd);
        return 0;
    }
    pass("l6_http_send");

    // Read response.
    size_t total = 0;
    char buf[4096];
    for (int i = 0; i < 500; i++) {
        struct pollfd pfd = { .fd = fd, .events = POLLIN };
        r = poll(&pfd, 1, 10000);
        if (r <= 0) break;
        ssize_t n = read(fd, buf, sizeof(buf));
        if (n <= 0) break;
        total += n;
    }
    printf("  received: %zu bytes\n", total);

    if (total > 1000)
        pass("l6_http_download");
    else {
        snprintf(detail, sizeof(detail), "only %zu bytes", total);
        fail("l6_http_download", detail);
        ok = 0;
    }

    close(fd);
    return ok;
}

// ─── Layer 7: Full apk update ────────────────────────────────────────────────

static int layer7_apk_update(void) {
    printf("LAYER 7: apk_update\n");
    char detail[256];

    char out[8192];
    char *argv[] = { "/sbin/apk", "update", NULL };
    int rc = chroot_exec_capture("/mnt", argv, out, sizeof(out), 30000);
    printf("  apk update exit=%d\n", rc);
    if (rc != 0) {
        snprintf(detail, sizeof(detail), "exit=%d out='%.80s'", rc, out);
        fail("l7_apk_update", detail);
        return 0;
    }
    pass("l7_apk_update_exit");

    // Verify APKINDEX was downloaded.
    DIR *d = opendir("/mnt/var/cache/apk");
    int found = 0;
    if (d) {
        struct dirent *ent;
        while ((ent = readdir(d)) != NULL) {
            if (strstr(ent->d_name, "APKINDEX")) {
                found = 1;
                break;
            }
        }
        closedir(d);
    }
    if (found)
        pass("l7_apkindex_cached");
    else
        fail("l7_apkindex_cached", "no APKINDEX in /var/cache/apk");

    return found;
}

// ─── Main ────────────────────────────────────────────────────────────────────

int main(void) {
    printf("TEST_START test_alpine\n");

    // Wait for DHCP to complete.
    sleep(3);

    // Layer 1: Foundation (ext2 mount + file checks)
    int l1 = layer1_foundation();

    // Layer 2: ext2 Write
    if (l1) {
        layer2_ext2_write();
    } else {
        const char *tests[] = { "l2_create_write_read", "l2_mkdir_rmdir",
                                "l2_symlink", "l2_rename", "l2_large_file" };
        skip_layer(2, "ext2_write", "layer 1 failed", tests, 5);
    }

    // Layer 3: chroot + Dynamic Linking
    int l3;
    if (l1) {
        l3 = layer3_chroot_dynlink();
    } else {
        const char *tests[] = { "l3_busybox_help", "l3_apk_version" };
        skip_layer(3, "chroot_dynlink", "layer 1 failed", tests, 2);
        l3 = 0;
    }

    // Layer 4: Alpine Package Database
    int l4;
    if (l3) {
        l4 = layer4_apk_database();
    } else {
        const char *tests[] = { "l4_apk_info", "l4_package_count" };
        skip_layer(4, "apk_database", "layer 3 failed", tests, 2);
        l4 = 0;
    }

    // Layer 5: DNS Resolution
    int l5;
    {
        l5 = layer5_dns();
    }
    if (!l5) {
        // Even if DNS fails, try layers 6-7 won't work
    }

    // Layer 6: TCP HTTP
    int l6;
    if (l5 && g_dns_ok) {
        l6 = layer6_tcp_http();
    } else {
        const char *tests[] = { "l6_tcp_socket", "l6_tcp_connect",
                                "l6_http_send", "l6_http_download" };
        skip_layer(6, "tcp_http", "layer 5 failed", tests, 4);
        l6 = 0;
    }

    // Layer 7: Full apk update
    if (l3 && l6) {
        layer7_apk_update();
    } else {
        const char *reason = !l3 ? "layer 3 failed" : "layer 6 failed";
        const char *tests[] = { "l7_apk_update_exit", "l7_apkindex_cached" };
        skip_layer(7, "apk_update", reason, tests, 2);
    }

    printf("test_alpine: %d passed, %d failed, %d skipped\n",
           g_pass, g_fail, g_skip);
    printf("TEST_END\n");
    return g_fail > 0 ? 1 : 0;
}
