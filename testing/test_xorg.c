// Test X11/Xorg startup on Kevlar with fbdev driver.
// Boots Alpine with X11 packages, attempts to start Xorg,
// and reports what works and what fails.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

static int g_pass, g_fail;

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

// Run a command in chroot, capture output, return exit code (-2 = timeout)
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
                         "HOME=/root", "TERM=vt100",
                         "DISPLAY=:0", NULL };
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
    int status = 0, waited = 0;
    for (int i = 0; i < timeout_ms / 100 + 1; i++) {
        pid_t w = waitpid(pid, &status, WNOHANG);
        if (w > 0) { waited = 1; break; }
        if (w < 0) break;
        usleep(100000);
    }
    if (!waited) { kill(pid, SIGKILL); waitpid(pid, &status, 0); return -2; }
    if (WIFEXITED(status)) return WEXITSTATUS(status);
    if (WIFSIGNALED(status)) return 128 + WTERMSIG(status);
    return -1;
}

#define ROOT "/mnt"
static char g_buf[8192];
static char g_detail[256];

int main(void) {
    printf("TEST_START test_xorg\n");
    printf("Kevlar X11/Xorg Integration Test\n");

    sleep(3); // Wait for DHCP

    // Mount Alpine X11 disk
    mkdir("/mnt", 0755);
    if (mount("/dev/vda", ROOT, "ext2", 0, NULL) != 0) {
        snprintf(g_detail, sizeof(g_detail), "errno=%d", errno);
        fail("mount_rootfs", g_detail);
        printf("TEST_END %d/%d\n", g_pass, g_pass + g_fail);
        return 1;
    }
    pass("mount_rootfs");

    // Mount virtual filesystems
    mkdir(ROOT "/proc", 0755);
    mount("proc", ROOT "/proc", "proc", 0, NULL);
    mkdir(ROOT "/sys", 0755);
    mount("sysfs", ROOT "/sys", "sysfs", 0, NULL);
    mkdir(ROOT "/dev", 0755);
    mount("devtmpfs", ROOT "/dev", "devtmpfs", 0, NULL);
    mkdir(ROOT "/dev/pts", 0755);
    mkdir(ROOT "/dev/shm", 0755);
    mkdir(ROOT "/dev/input", 0755);
    mkdir(ROOT "/tmp", 01777);
    mount("tmpfs", ROOT "/tmp", "tmpfs", 0, NULL);
    mkdir(ROOT "/run", 0755);
    mount("tmpfs", ROOT "/run", "tmpfs", 0, NULL);

    // Set up DNS
    {
        int fd = open(ROOT "/etc/resolv.conf", O_WRONLY | O_CREAT | O_TRUNC, 0644);
        if (fd >= 0) { write(fd, "nameserver 10.0.2.3\n", 20); close(fd); }
    }

    // Phase 1: Check device files
    printf("\n=== Phase 1: Device Files ===\n");

    // Check /dev/fb0
    {
        int fd = open(ROOT "/dev/fb0", O_RDWR);
        if (fd >= 0) {
            pass("dev_fb0_open");
            // Try FBIOGET_VSCREENINFO
            unsigned char vinfo[160] = {0};
            if (ioctl(fd, 0x4600, vinfo) == 0) {
                unsigned int *v = (unsigned int *)vinfo;
                printf("  fb0: %ux%u %ubpp\n", v[0], v[1], v[6]);
                if (v[0] > 0 && v[1] > 0 && v[6] == 32)
                    pass("fb0_vscreeninfo");
                else
                    fail("fb0_vscreeninfo", "unexpected values");
            } else {
                fail("fb0_vscreeninfo", "ioctl failed");
            }
            // Try mmap
            unsigned char finfo[68] = {0};
            if (ioctl(fd, 0x4602, finfo) == 0) {
                unsigned int smem_len = *(unsigned int *)(finfo + 24);
                void *fb = mmap(NULL, smem_len, PROT_READ | PROT_WRITE,
                                MAP_SHARED, fd, 0);
                if (fb != MAP_FAILED) {
                    // Write a test pixel
                    ((unsigned int *)fb)[0] = 0xFFFF0000; // Red pixel
                    pass("fb0_mmap");
                    munmap(fb, smem_len);
                } else {
                    snprintf(g_detail, sizeof(g_detail), "errno=%d", errno);
                    fail("fb0_mmap", g_detail);
                }
            } else {
                fail("fb0_mmap", "FSCREENINFO ioctl failed");
            }
            close(fd);
        } else {
            snprintf(g_detail, sizeof(g_detail), "errno=%d", errno);
            fail("dev_fb0_open", g_detail);
        }
    }

    // Check /dev/input/mice
    {
        struct stat st;
        if (stat(ROOT "/dev/input/mice", &st) == 0)
            pass("dev_input_mice");
        else
            fail("dev_input_mice", "not found");
    }

    // Phase 2: Check X11 binaries
    printf("\n=== Phase 2: X11 Binaries ===\n");
    {
        struct { const char *path; const char *name; } bins[] = {
            { ROOT "/usr/bin/Xorg",     "xorg_binary" },
            { ROOT "/usr/bin/xterm",    "xterm_binary" },
            { ROOT "/usr/bin/twm",      "twm_binary" },
            { ROOT "/usr/bin/xinit",    "xinit_binary" },
            { ROOT "/usr/bin/xdpyinfo", "xdpyinfo_binary" },
        };
        for (int i = 0; i < 5; i++) {
            struct stat st;
            if (stat(bins[i].path, &st) == 0 && (st.st_mode & S_IXUSR))
                pass(bins[i].name);
            else
                fail(bins[i].name, "not found or not executable");
        }
    }

    // Phase 3: Check X11 config
    printf("\n=== Phase 3: X11 Config ===\n");
    {
        struct stat st;
        if (stat(ROOT "/etc/X11/xorg.conf.d/10-fbdev.conf", &st) == 0)
            pass("xorg_fbdev_conf");
        else
            fail("xorg_fbdev_conf", "not found");
    }

    // Phase 4: Attempt Xorg startup
    printf("\n=== Phase 4: Xorg Startup ===\n");

    // First check if Xorg can at least run (show help)
    {
        int rc = sh_exec(ROOT,
            "ls -la /usr/libexec/Xorg /usr/bin/Xorg 2>&1; "
            "ldd /usr/libexec/Xorg 2>&1 | head -10; "
            "/usr/libexec/Xorg -version 2>&1 | head -5",
            g_buf, sizeof(g_buf), 10000);
        printf("  xorg info: %.300s\n", g_buf);

        if (strstr(g_buf, "X.Org") || strstr(g_buf, "X Window"))
            pass("xorg_version");
        else {
            snprintf(g_detail, sizeof(g_detail), "rc=%d", rc);
            fail("xorg_version", g_detail);
        }
    }

    // Try starting Xorg with fbdev
    {
        int rc = sh_exec(ROOT,
            "/usr/libexec/Xorg :0 -noreset -nolisten tcp -config /etc/X11/xorg.conf.d/10-fbdev.conf 2>&1 &"
            "XPID=$!; sleep 3; "
            "if kill -0 $XPID 2>/dev/null; then "
            "  echo XORG_RUNNING; "
            "  DISPLAY=:0 xdpyinfo 2>&1 | head -5; "
            "  kill $XPID 2>/dev/null; wait $XPID 2>/dev/null; "
            "else "
            "  echo XORG_FAILED; wait $XPID 2>/dev/null; echo exit=$?; "
            "fi",
            g_buf, sizeof(g_buf), 20000);

        printf("  xorg output: %.400s\n", g_buf);

        if (strstr(g_buf, "XORG_RUNNING"))
            pass("xorg_startup");
        else {
            snprintf(g_detail, sizeof(g_detail), "rc=%d", rc);
            fail("xorg_startup", g_detail);
        }

        if (strstr(g_buf, "display") || strstr(g_buf, "screen"))
            pass("xdpyinfo");
        else
            fail("xdpyinfo", "no display info");
    }

    printf("\n");
    printf("X11 TEST: %d passed, %d failed\n", g_pass, g_fail);
    printf("TEST_END %d/%d\n", g_pass, g_pass + g_fail);
    return g_fail > 0 ? 1 : 0;
}
