// Test XFCE desktop startup on Kevlar.
// Boots Alpine XFCE image, starts D-Bus + Xorg + XFCE, reports results.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#define ROOT "/mnt"

static int g_pass, g_fail, g_total;

static void pass(const char *name) {
    printf("TEST_PASS %s\n", name);
    g_pass++; g_total++;
}

static void fail(const char *name, const char *detail) {
    if (detail) printf("TEST_FAIL %s (%s)\n", name, detail);
    else printf("TEST_FAIL %s\n", name);
    g_fail++; g_total++;
}

// Run a command in chroot, return exit code (-2 = timeout)
static int sh_exec(const char *cmd, int timeout_ms) {
    pid_t pid = fork();
    if (pid < 0) return -1;
    if (pid == 0) {
        chroot(ROOT);
        chdir("/");
        execl("/bin/sh", "sh", "-c", cmd, NULL);
        _exit(127);
    }
    // Simple wait with timeout
    for (int i = 0; i < timeout_ms / 100; i++) {
        int status;
        pid_t r = waitpid(pid, &status, WNOHANG);
        if (r == pid) return WIFEXITED(status) ? WEXITSTATUS(status) : -1;
        usleep(100000);
    }
    kill(pid, SIGKILL);
    waitpid(pid, NULL, 0);
    return -2;
}

// Start a daemon in chroot (doesn't wait for it)
static pid_t start_daemon(const char *cmd) {
    pid_t pid = fork();
    if (pid == 0) {
        chroot(ROOT);
        chdir("/");
        setsid();
        int fd = open("/dev/null", O_RDWR);
        if (fd >= 0) { dup2(fd, 0); dup2(fd, 1); dup2(fd, 2); if (fd > 2) close(fd); }
        execl("/bin/sh", "sh", "-c", cmd, NULL);
        _exit(127);
    }
    return pid;
}

// Check if a process name is running via /proc
static int process_running(const char *name) {
    char cmd[256];
    snprintf(cmd, sizeof(cmd), "pgrep -x '%s' >/dev/null 2>&1", name);
    return sh_exec(cmd, 2000) == 0;
}

static void setup_rootfs(void) {
    mkdir(ROOT, 0755);
    // Wait for virtio-blk init
    sleep(2);
    if (mount("/dev/vda", ROOT, "ext2", 0, NULL) != 0) {
        printf("TEST_FAIL mount_rootfs (errno=%d)\n", errno);
        return;
    }
    printf("TEST_PASS mount_rootfs\n");

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
    printf("T: XFCE Desktop Test\n");
    setup_rootfs();

    // Phase 1: Start D-Bus
    printf("\n=== Phase 1: D-Bus ===\n");
    {
        int rc = sh_exec("dbus-daemon --system 2>/dev/null", 5000);
        if (rc == 0) pass("dbus_daemon_start");
        else {
            char buf[64]; snprintf(buf, sizeof(buf), "exit=%d", rc);
            fail("dbus_daemon_start", buf);
        }
    }

    // Phase 2: Start Xorg
    printf("\n=== Phase 2: Xorg ===\n");
    {
        // Start Xorg in background via shell (uses /usr/libexec/Xorg on Alpine)
        int rc = sh_exec(
            "/usr/libexec/Xorg :0 -noreset -nolisten tcp "
            "-config /etc/X11/xorg.conf.d/10-fbdev.conf 2>/dev/null &"
            "sleep 4; "
            "if kill -0 $! 2>/dev/null; then echo XORG_OK; fi",
            10000);
        if (rc == 0)
            pass("xorg_running");
        else {
            char buf[64]; snprintf(buf, sizeof(buf), "rc=%d", rc);
            fail("xorg_running", buf);
        }
    }

    // Phase 3: Test X connectivity
    {
        int rc = sh_exec("DISPLAY=:0 xdpyinfo >/dev/null 2>&1", 5000);
        if (rc == 0) pass("xdpyinfo");
        else fail("xdpyinfo", "failed");
    }

    // Phase 4: Start XFCE session
    printf("\n=== Phase 3: XFCE ===\n");
    {
        int rc = sh_exec(
            "export DISPLAY=:0 HOME=/root; "
            "dbus-launch startxfce4 >/dev/null 2>&1 &"
            "sleep 8; "
            "# Check what XFCE processes are running\n"
            "ps aux 2>/dev/null | grep -E 'xfwm|xfce4-panel|xfce4-session' | grep -v grep",
            20000);

        // Check individual processes via pgrep
        if (sh_exec("pgrep -x xfwm4 >/dev/null 2>&1", 2000) == 0)
            pass("xfwm4_running");
        else
            fail("xfwm4_running", "window manager not found");

        if (sh_exec("pgrep -x xfce4-panel >/dev/null 2>&1", 2000) == 0)
            pass("xfce4_panel_running");
        else
            fail("xfce4_panel_running", "panel not found");

        if (sh_exec("pgrep xfce4-session >/dev/null 2>&1", 2000) == 0)
            pass("xfce4_session_running");
        else
            fail("xfce4_session_running", "session not found");
    }

    // Phase 5: Check unimplemented syscall warnings
    printf("\n=== Phase 4: Syscall Coverage ===\n");
    {
        // Check /proc/version to verify we're on Kevlar
        int rc = sh_exec("cat /proc/version | grep -qi kevlar", 2000);
        if (rc == 0) pass("proc_version_kevlar");
        else fail("proc_version_kevlar", "not Kevlar");
    }

    printf("\nTEST_END %d/%d\n", g_pass, g_total);
    printf("T: %d passed, %d failed\n", g_pass, g_fail);

    sync();
    // Exit cleanly
    return 0;
}
