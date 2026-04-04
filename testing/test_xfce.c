// Fully automated end-to-end XFCE desktop test for Kevlar.
// Boots Alpine XFCE image, sets up D-Bus, starts Xorg + XFCE, reports results.
// Run via: make test-xfce
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
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

#define ROOT "/mnt"

static int g_pass, g_fail, g_total;

static void pass(const char *name) {
    printf("TEST_PASS %s\n", name);
    fflush(stdout);
    g_pass++; g_total++;
}

static void fail(const char *name, const char *detail) {
    if (detail) printf("TEST_FAIL %s (%s)\n", name, detail);
    else printf("TEST_FAIL %s\n", name);
    fflush(stdout);
    g_fail++; g_total++;
}

// Run a command in chroot, return exit code (-2 = timeout)
static int sh_run(const char *cmd, int timeout_ms) {
    pid_t pid = fork();
    if (pid < 0) return -1;
    if (pid == 0) {
        if (chroot(ROOT) != 0) _exit(126);
        chdir("/");
        execl("/bin/sh", "sh", "-c", cmd, NULL);
        _exit(127);
    }
    for (int i = 0; i < timeout_ms / 50; i++) {
        int status;
        pid_t r = waitpid(pid, &status, WNOHANG);
        if (r == pid) return WIFEXITED(status) ? WEXITSTATUS(status) : -1;
        usleep(50000);
    }
    kill(pid, SIGKILL);
    waitpid(pid, NULL, 0);
    return -2;
}

// Run a command in chroot, capture stdout to caller's buffer
static int sh_capture(const char *cmd, char *buf, int bufsz, int timeout_ms) {
    int pipefd[2];
    if (pipe(pipefd) < 0) return -1;
    pid_t pid = fork();
    if (pid < 0) { close(pipefd[0]); close(pipefd[1]); return -1; }
    if (pid == 0) {
        close(pipefd[0]);
        dup2(pipefd[1], 1);
        dup2(pipefd[1], 2);
        close(pipefd[1]);
        if (chroot(ROOT) != 0) _exit(126);
        chdir("/");
        execl("/bin/sh", "sh", "-c", cmd, NULL);
        _exit(127);
    }
    close(pipefd[1]);
    int total = 0;
    for (int i = 0; i < timeout_ms / 50; i++) {
        char tmp[256];
        int n = read(pipefd[0], tmp, sizeof(tmp));
        if (n > 0 && total + n < bufsz) { memcpy(buf + total, tmp, n); total += n; }
        int status;
        pid_t r = waitpid(pid, &status, WNOHANG);
        if (r == pid) break;
        usleep(50000);
    }
    close(pipefd[0]);
    buf[total < bufsz ? total : bufsz - 1] = '\0';
    kill(pid, SIGKILL);
    waitpid(pid, NULL, 0);
    return total > 0 ? 0 : -1;
}

// Start a daemon in chroot (doesn't wait)
static pid_t start_bg(const char *cmd) {
    pid_t pid = fork();
    if (pid == 0) {
        if (chroot(ROOT) != 0) _exit(126);
        chdir("/");
        setsid();
        int fd = open("/dev/null", O_RDWR);
        if (fd >= 0) { dup2(fd, 0); if (fd > 2) close(fd); }
        // Keep stdout/stderr for Xorg logging to serial
        execl("/bin/sh", "sh", "-c", cmd, NULL);
        _exit(127);
    }
    return pid;
}

static void setup_rootfs(void) {
    mkdir(ROOT, 0755);
    sleep(2); // Wait for virtio-blk
    if (mount("/dev/vda", ROOT, "ext2", 0, NULL) != 0) {
        char buf[64]; snprintf(buf, sizeof(buf), "errno=%d", errno);
        fail("mount_rootfs", buf);
        printf("TEST_END 0/1\n");
        _exit(1);
    }
    pass("mount_rootfs");

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
    mkdir(ROOT "/run/dbus", 0755);
    mkdir(ROOT "/tmp/.X11-unix", 01777);
}

int main(void) {
    printf("T: XFCE Desktop End-to-End Test\n");
    fflush(stdout);
    setup_rootfs();

    // --- Phase 1: Check /dev/fb0 ---
    printf("\n=== Phase 1: Framebuffer ===\n");
    {
        char buf[256];
        if (sh_capture("ls -la /dev/fb0 2>&1", buf, sizeof(buf), 2000) == 0) {
            printf("  %s", buf);
            pass("dev_fb0_exists");
        } else {
            fail("dev_fb0_exists", "not found — need -vga std");
        }
        // Check fb0 ioctl
        // Test fb0 ioctl directly (outside chroot, using kernel's /dev/fb0)
        {
            int fd = open("/dev/fb0", O_RDWR);
            if (fd >= 0) {
                unsigned char vinfo[160] = {0};
                if (ioctl(fd, 0x4600, vinfo) == 0) {
                    unsigned int *v = (unsigned int *)vinfo;
                    printf("  fb0 ioctl OK: %ux%u %ubpp\n", v[0], v[1], v[6]);
                    pass("fb0_ioctl");
                } else {
                    printf("  fb0 VSCREENINFO FAILED: errno=%d\n", errno);
                    fail("fb0_ioctl", "FBIOGET_VSCREENINFO failed");
                }
                // Also test FBIOGET_FSCREENINFO (0x4602)
                unsigned char finfo[68] = {0};
                if (ioctl(fd, 0x4602, finfo) == 0) {
                    unsigned int smem_len = *(unsigned int *)(finfo + 24);
                    unsigned int line_length = *(unsigned int *)(finfo + 48);
                    printf("  fb0 FSCREENINFO: smem_len=%u line_length=%u\n", smem_len, line_length);
                } else {
                    printf("  fb0 FSCREENINFO FAILED: errno=%d\n", errno);
                }
                // Test mmap
                unsigned char finfo2[68] = {0};
                if (ioctl(fd, 0x4602, finfo2) == 0) {
                    unsigned int smem_len = *(unsigned int *)(finfo2 + 24);
                    void *fb = mmap(NULL, smem_len, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
                    if (fb != MAP_FAILED) {
                        printf("  fb0 mmap OK at %p (%u bytes)\n", fb, smem_len);
                        munmap(fb, smem_len);
                    } else {
                        printf("  fb0 mmap FAILED: errno=%d\n", errno);
                    }
                }
                close(fd);
            } else {
                printf("  fb0 open FAILED: errno=%d\n", errno);
                fail("fb0_ioctl", "can't open /dev/fb0");
            }
        }
    }

    // --- Phase 2: D-Bus ---
    printf("\n=== Phase 2: D-Bus ===\n");
    {
        sh_run("rm -f /run/dbus/dbus.pid", 1000);
        sh_run("dbus-uuidgen --ensure 2>/dev/null", 2000);
        int rc = sh_run("dbus-daemon --system 2>/dev/null", 5000);
        if (rc == 0) pass("dbus_start");
        else { char b[32]; snprintf(b, sizeof(b), "rc=%d", rc); fail("dbus_start", b); }
    }

    // --- Phase 3: Start Xorg ---
    printf("\n=== Phase 3: Xorg ===\n");
    {
        // Clean stale locks
        sh_run("rm -f /tmp/.X0-lock /tmp/.X11-unix/X0", 1000);

        // Start Xorg in background
        start_bg("/usr/libexec/Xorg :0 -noreset -nolisten tcp "
                 "-config /etc/X11/xorg.conf.d/10-fbdev.conf vt1 2>&1");
        sleep(4);

        // Dump Xorg diagnostic info
        sleep(2);
        {
            char buf[2048];
            // Test fb0 ioctl from INSIDE the chroot
            {
                char b[256];
                sh_capture("ls -la /dev/fb0 2>&1", b, sizeof(b), 2000);
                printf("  chroot fb0: %s", b);
                int rc2 = sh_run("dd if=/dev/fb0 bs=4 count=1 of=/dev/null 2>/dev/null", 3000);
                printf("  chroot fb0 read: %s\n", rc2 == 0 ? "OK" : "FAILED");
                // Check sysfs — Xorg fbdev may need /sys/class/graphics/fb0
                sh_capture("ls -laR /sys/class/graphics/ 2>&1 || echo 'no sysfs graphics'", b, sizeof(b), 2000);
                printf("  sysfs graphics: %s\n", b);
                // Also check from outside chroot
                {
                    struct stat sst;
                    int rc = stat(ROOT "/sys/class/graphics/fb0", &sst);
                    printf("  sysfs fb0 stat: %s (rc=%d errno=%d)\n", rc == 0 ? "EXISTS" : "MISSING", rc, errno);
                    rc = stat(ROOT "/sys/class/graphics/fb0/dev", &sst);
                    printf("  sysfs fb0/dev stat: %s (rc=%d)\n", rc == 0 ? "EXISTS" : "MISSING", rc);
                }
            }
            // Check fbdev driver module exists
            printf("  fbdev driver: ");
            sh_capture("ls /usr/lib/xorg/modules/drivers/fbdev_drv.so 2>&1", buf, sizeof(buf), 2000);
            printf("%s", buf);

            // Dump Xorg log error lines
            printf("  Xorg errors:\n");
            sh_capture("grep '(EE)' /var/log/Xorg.0.log 2>/dev/null", buf, sizeof(buf), 2000);
            printf("  %s", buf);

            // Dump fbdev-related log lines
            printf("  Xorg fbdev lines:\n");
            sh_capture("grep -i 'fbdev\\|fb0\\|FBIO\\|framebuffer\\|screen.*added\\|No devices' /var/log/Xorg.0.log 2>/dev/null", buf, sizeof(buf), 2000);
            printf("  %s\n", buf);
            fflush(stdout);
        }

        // Check if running
        if (sh_run("kill -0 $(cat /tmp/.X0-lock 2>/dev/null) 2>/dev/null", 2000) == 0) {
            pass("xorg_running");
        } else {
            fail("xorg_running", "not found");
        }

        // Check display connectivity
        int rc = sh_run("DISPLAY=:0 xdpyinfo >/dev/null 2>&1", 5000);
        if (rc == 0) pass("xdpyinfo");
        else {
            char b[32]; snprintf(b, sizeof(b), "rc=%d", rc);
            fail("xdpyinfo", b);
        }
    }

    // --- Phase 4: Start XFCE ---
    printf("\n=== Phase 4: XFCE Session ===\n");
    {
        start_bg("export DISPLAY=:0 HOME=/root; "
                 "dbus-launch startxfce4 2>&1");
        printf("  Waiting 10s for XFCE to initialize...\n");
        fflush(stdout);
        sleep(10);

        // Check components
        if (sh_run("pgrep -x xfwm4 >/dev/null 2>&1", 2000) == 0)
            pass("xfwm4_running");
        else
            fail("xfwm4_running", "window manager not found");

        if (sh_run("pgrep -x xfce4-panel >/dev/null 2>&1", 2000) == 0)
            pass("xfce4_panel_running");
        else
            fail("xfce4_panel_running", "panel not found");

        if (sh_run("pgrep xfce4-session >/dev/null 2>&1", 2000) == 0)
            pass("xfce4_session_running");
        else
            fail("xfce4_session_running", "session not found");

        // List all running processes for diagnostics
        char buf[4096];
        printf("  Running processes:\n");
        sh_capture("ps aux 2>/dev/null | head -30", buf, sizeof(buf), 3000);
        printf("%s\n", buf);

        // Check for unimplemented syscall warnings
        printf("  Unimplemented syscalls encountered:\n");
        sh_capture("dmesg 2>/dev/null | grep 'unimplemented' | sort -u | head -10",
                  buf, sizeof(buf), 2000);
        if (buf[0]) printf("%s\n", buf);
        else printf("  (none)\n");
    }

    printf("\nTEST_END %d/%d\n", g_pass, g_total);
    printf("T: %d passed, %d failed\n", g_pass, g_fail);
    fflush(stdout);

    sync();
    return 0;
}
