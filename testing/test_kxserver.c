// Test kxserver boot: mount the Alpine rootfs, chroot, run kxserver, verify
// it starts, parses its CLI, and binds both the filesystem and abstract
// Unix socket listeners for display :1.
//
// Phase 1 of the kxserver project.  Subsequent phases will extend this
// harness to drive an in-chroot client through the ConnectionSetup
// handshake and beyond.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <unistd.h>

static int g_pass, g_fail;
static char g_buf[8192];

#define ROOT "/mnt"

static void pass(const char *name) {
    printf("TEST_PASS %s\n", name);
    g_pass++;
}

static void fail(const char *name, const char *detail) {
    if (detail)
        printf("TEST_FAIL %s (%s)\n", name, detail);
    else
        printf("TEST_FAIL %s\n", name);
    g_fail++;
}

static int sh_exec(const char *root, const char *cmd, char *out, int outsz, int timeout_ms) {
    int pipefd[2];
    if (pipe(pipefd) < 0) return -1;
    pid_t pid = fork();
    if (pid < 0) { close(pipefd[0]); close(pipefd[1]); return -1; }
    if (pid == 0) {
        close(pipefd[0]);
        dup2(pipefd[1], 1);
        dup2(pipefd[1], 2);
        close(pipefd[1]);
        if (chroot(root) < 0) _exit(126);
        chdir("/");
        char *envp[] = { "PATH=/usr/sbin:/usr/bin:/sbin:/bin",
                         "HOME=/root", "TERM=vt100", NULL };
        char *argv[] = { "/bin/sh", "-c", (char *)cmd, NULL };
        execve("/bin/sh", argv, envp);
        _exit(127);
    }
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
    int status;
    waitpid(pid, &status, 0);
    if (WIFEXITED(status)) return WEXITSTATUS(status);
    return -1;
}

static void mount_rootfs(void) {
    mkdir("/mnt", 0755);
    mkdir("/dev", 0755);
    mount("devtmpfs", "/dev", "devtmpfs", 0, "");

    if (mount("/dev/vda", ROOT, "ext2", 0, NULL) != 0)
        mount("/dev/vda", ROOT, "ext4", 0, NULL);

    mkdir(ROOT "/proc", 0755);
    mount("proc", ROOT "/proc", "proc", 0, NULL);
    mkdir(ROOT "/sys", 0755);
    mount("sysfs", ROOT "/sys", "sysfs", 0, NULL);
    mkdir(ROOT "/dev", 0755);
    mount("devtmpfs", ROOT "/dev", "devtmpfs", 0, NULL);
    mkdir(ROOT "/dev/pts", 0755);
    mount("devpts", ROOT "/dev/pts", "devpts", 0, NULL);
    mkdir(ROOT "/tmp", 0777);
    mount("tmpfs", ROOT "/tmp", "tmpfs", 0, "mode=1777");
    mkdir(ROOT "/run", 0755);
    mount("tmpfs", ROOT "/run", "tmpfs", 0, NULL);
}

int main(void) {
    printf("TEST_START test_kxserver\n");

    sleep(2); // Wait for device init

    printf("\n=== Phase 1: Mount Rootfs ===\n");
    mount_rootfs();
    {
        struct stat st;
        if (stat(ROOT "/usr/bin/kxserver", &st) == 0)
            pass("mount_rootfs");
        else {
            fail("mount_rootfs", "kxserver not found in rootfs");
            goto done;
        }
    }

    printf("\n=== Phase 2: kxserver --help ===\n");
    {
        int rc = sh_exec(ROOT,
            "/usr/bin/kxserver --help 2>&1 | head -20",
            g_buf, sizeof(g_buf), 5000);
        printf("  kxserver --help output:\n%s\n", g_buf);
        if (rc == 0 && strstr(g_buf, "kxserver"))
            pass("kxserver_help");
        else
            fail("kxserver_help", g_buf);
    }

    printf("\n=== Phase 3: kxserver listens on :1 ===\n");
    {
        int rc = sh_exec(ROOT,
            "/usr/bin/kxserver :1 --log=req >/tmp/kx.log 2>&1 &"
            "KX=$!; sleep 1; "
            "kill $KX 2>/dev/null; wait 2>/dev/null; "
            "cat /tmp/kx.log 2>/dev/null || echo '(no log)'",
            g_buf, sizeof(g_buf), 8000);
        printf("  kxserver listen output:\n%s\n", g_buf);
        if (strstr(g_buf, "listening (filesystem) on /tmp/.X11-unix/X1"))
            pass("kxserver_listen");
        else
            fail("kxserver_listen", "no listen log");
    }

    printf("\n=== Phase 4: kxserver dual-bind (fs + abstract) ===\n");
    {
        int rc = sh_exec(ROOT,
            "/usr/bin/kxserver :1 --log=req >/tmp/kx.log 2>&1 &"
            "KX=$!; sleep 1; "
            "kill $KX 2>/dev/null; wait 2>/dev/null; "
            "cat /tmp/kx.log 2>/dev/null",
            g_buf, sizeof(g_buf), 8000);
        int has_fs   = strstr(g_buf, "listening (filesystem)") != NULL;
        int has_abs  = strstr(g_buf, "listening (abstract)")   != NULL;
        int no_panic = strstr(g_buf, "FATAL") == NULL &&
                       strstr(g_buf, "panic") == NULL;
        printf("  filesystem listener: %d, abstract listener: %d, no panic: %d\n",
               has_fs, has_abs, no_panic);
        if (has_fs && has_abs && no_panic)
            pass("kxserver_bind_dual");
        else
            fail("kxserver_bind_dual", "missing listener log lines");
    }

    printf("\n=== Phase 5: raw X11 handshake over filesystem socket ===\n");
    {
        // Fork a chrooted child that starts kxserver, connects to its
        // filesystem socket path, sends a 12-byte handshake, and reads
        // back the 8-byte reply header.  Child exits 0 on success.
        pid_t harness = fork();
        if (harness == 0) {
            if (chroot(ROOT) < 0) _exit(126);
            chdir("/");

            pid_t kx = fork();
            if (kx == 0) {
                char *envp[] = { "PATH=/usr/bin", NULL };
                char *argv[] = { "/usr/bin/kxserver", ":1", "--log=req", NULL };
                int fd = open("/tmp/kx.log", O_WRONLY | O_CREAT | O_TRUNC, 0644);
                if (fd >= 0) { dup2(fd, 1); dup2(fd, 2); close(fd); }
                execve("/usr/bin/kxserver", argv, envp);
                _exit(127);
            }
            usleep(500000);  // let kxserver bind

            int sock = socket(AF_UNIX, SOCK_STREAM, 0);
            if (sock < 0) { kill(kx, 9); waitpid(kx, NULL, 0); _exit(1); }

            struct sockaddr_un sun;
            memset(&sun, 0, sizeof(sun));
            sun.sun_family = AF_UNIX;
            strcpy(sun.sun_path, "/tmp/.X11-unix/X1");
            socklen_t alen = (socklen_t)(sizeof(sa_family_t)
                                         + strlen("/tmp/.X11-unix/X1") + 1);

            int r = -1;
            for (int retry = 0; retry < 10 && r != 0; retry++) {
                r = connect(sock, (struct sockaddr *)&sun, alen);
                if (r != 0) usleep(200000);
            }
            if (r != 0) {
                printf("p5.fail connect errno=%d\n", errno);
                close(sock);
                kill(kx, 9); waitpid(kx, NULL, 0);
                _exit(2);
            }

            unsigned char hs[12] = { 0x6C, 0, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0 };
            ssize_t wrote = write(sock, hs, sizeof(hs));
            if (wrote != 12) {
                printf("p5.fail write=%zd errno=%d\n", wrote, errno);
                close(sock);
                kill(kx, 9); waitpid(kx, NULL, 0);
                _exit(3);
            }

            unsigned char rep[8];
            ssize_t got = 0;
            while (got < 8) {
                struct pollfd pfd = { .fd = sock, .events = POLLIN };
                int pr = poll(&pfd, 1, 3000);
                if (pr <= 0) break;
                ssize_t n = read(sock, rep + got, 8 - got);
                if (n <= 0) break;
                got += n;
            }
            printf("p5.reply head:");
            for (int i = 0; i < got; i++) printf(" %02x", rep[i]);
            printf("\n");

            close(sock);
            usleep(100000);
            kill(kx, 9);
            waitpid(kx, NULL, 0);

            int ok = (got == 8 && rep[0] == 1 && rep[2] == 11 && rep[3] == 0);
            printf("p5.server_log_tail:\n");
            int lf = open("/tmp/kx.log", O_RDONLY);
            if (lf >= 0) {
                char b[2048];
                ssize_t ln;
                while ((ln = read(lf, b, sizeof(b))) > 0) {
                    fwrite(b, 1, ln, stdout);
                }
                close(lf);
            }
            fflush(stdout);
            _exit(ok ? 0 : 4);
        }

        int status;
        waitpid(harness, &status, 0);
        int ok = WIFEXITED(status) && WEXITSTATUS(status) == 0;
        printf("  harness exit: %d\n",
               WIFEXITED(status) ? WEXITSTATUS(status) : -1);
        if (ok)
            pass("kxserver_handshake");
        else
            fail("kxserver_handshake", "handshake reply wrong or missing");
    }

    printf("\n=== Phase 6: kxserver binary metadata ===\n");
    {
        int rc = sh_exec(ROOT,
            "ls -l /usr/bin/kxserver",
            g_buf, sizeof(g_buf), 5000);
        printf("  ls output:\n%s\n", g_buf);
        if (strstr(g_buf, "kxserver"))
            pass("kxserver_binary");
        else
            fail("kxserver_binary", g_buf);
    }

done:
    printf("\n");
    printf("KXSERVER PHASE-0 TEST: %d passed, %d failed\n", g_pass, g_fail);
    printf("TEST_END %d/%d\n", g_pass, g_pass + g_fail);
    return g_fail > 0 ? 1 : 0;
}
