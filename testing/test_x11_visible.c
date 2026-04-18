// Minimal Kevlar X11 screenshot target:
//   1. mount Alpine rootfs
//   2. start Xorg (fbdev)
//   3. xsetroot -solid (blue background)
//   4. xterm -e 'echo HELLO_KEVLAR; sleep forever'
//   5. idle forever so an external screenshotter can capture.
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

static int sh_run(const char *cmd, int timeout_ms) {
    pid_t pid = fork();
    if (pid < 0) return -1;
    if (pid == 0) {
        chroot(ROOT);
        chdir("/");
        char *envp[] = { "PATH=/usr/bin:/usr/sbin:/sbin:/bin",
                         "HOME=/root", "TERM=vt100", "DISPLAY=:0", NULL };
        char *argv[] = { "/bin/sh", "-c", (char *)cmd, NULL };
        execve("/bin/sh", argv, envp);
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

static void start_bg(const char *cmd) {
    pid_t pid = fork();
    if (pid < 0) return;
    if (pid == 0) {
        chroot(ROOT);
        chdir("/");
        char *envp[] = { "PATH=/usr/bin:/usr/sbin:/sbin:/bin",
                         "HOME=/root", "TERM=vt100", "DISPLAY=:0", NULL };
        char *argv[] = { "/bin/sh", "-c", (char *)cmd, NULL };
        execve("/bin/sh", argv, envp);
        _exit(127);
    }
    // Parent continues immediately.
}

int main(void) {
    printf("X11_VISIBLE: start\n"); fflush(stdout);

    // Mount alpine rootfs from virtio-blk disk.
    mkdir(ROOT, 0755);
    if (mount("/dev/vda1", ROOT, "ext4", MS_RELATIME, NULL) != 0) {
        // Try without partition
        if (mount("/dev/vda", ROOT, "ext4", MS_RELATIME, NULL) != 0) {
            printf("X11_VISIBLE: rootfs mount FAILED errno=%d\n", errno);
            fflush(stdout);
            while (1) sleep(60);
        }
    }
    printf("X11_VISIBLE: rootfs mounted\n"); fflush(stdout);

    // Mount /dev, /proc, /sys inside chroot.
    mount("/dev", ROOT "/dev", NULL, MS_BIND, NULL);
    mount("/proc", ROOT "/proc", NULL, MS_BIND, NULL);
    mount("/sys", ROOT "/sys", NULL, MS_BIND, NULL);

    // Clean stale X lock files.
    sh_run("rm -f /tmp/.X0-lock /tmp/.X11-unix/X0", 1000);

    // Generate font dirs Xorg needs.
    sh_run("mkfontscale /usr/share/fonts/misc 2>/dev/null; "
           "mkfontdir /usr/share/fonts/misc 2>/dev/null", 10000);

    // Start Xorg.
    printf("X11_VISIBLE: starting Xorg...\n"); fflush(stdout);
    start_bg("/usr/libexec/Xorg :0 -noreset -nolisten tcp "
             "-config /etc/X11/xorg.conf.d/10-fbdev.conf "
             "vt1 >/tmp/xorg-stdout.log 2>&1");
    sleep(5);

    // Check Xorg is alive.
    int rc = sh_run("pgrep -x Xorg >/dev/null 2>&1", 2000);
    printf("X11_VISIBLE: Xorg alive=%d\n", rc == 0); fflush(stdout);

    // Paint blue background.
    rc = sh_run("xsetroot -solid '#336699' 2>&1", 5000);
    printf("X11_VISIBLE: xsetroot rc=%d\n", rc); fflush(stdout);

    // Start xterm with a visible message. Use bitmap fonts only (no Xft)
    // to avoid fontconfig dependencies that may not be set up on this
    // minimal Alpine image.
    start_bg("xterm -geometry 90x28+50+50 "
             "-bg '#001122' -fg '#ffcc00' "
             "+ls "
             "-title 'Kevlar Kernel' "
             "-e /bin/sh -c 'clear; "
             "echo; echo \"  ================================\"; "
             "echo \"      HELLO FROM KEVLAR KERNEL\"; "
             "echo \"  ================================\"; "
             "echo; echo \"  Rust kernel running Alpine Linux\"; "
             "echo \"  Xorg + xterm rendering to /dev/fb0\"; "
             "echo; hostname; uname -a; "
             "sleep 3600' 2>/tmp/xterm.err");
    sleep(4);
    rc = sh_run("pgrep -x xterm >/dev/null 2>&1", 2000);
    printf("X11_VISIBLE: xterm alive=%d\n", rc == 0); fflush(stdout);
    if (rc != 0) {
        char b[512];
        int p[2]; pipe(p);
        pid_t pid = fork();
        if (pid == 0) {
            close(p[0]); dup2(p[1], 1); dup2(p[1], 2); close(p[1]);
            if (chroot(ROOT) == 0) { chdir("/"); }
            execlp("cat", "cat", "/tmp/xterm.err", NULL);
            _exit(127);
        }
        close(p[1]);
        int n = read(p[0], b, sizeof(b) - 1);
        if (n > 0) { b[n] = 0; printf("  xterm.err: %s\n", b); }
        close(p[0]); waitpid(pid, NULL, 0);
    }

    printf("X11_VISIBLE: ready for screenshots, sleeping forever\n"); fflush(stdout);
    while (1) sleep(60);
}
