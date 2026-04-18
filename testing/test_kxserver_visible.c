// Boot Kevlar, mount alpine-xorg rootfs, start kxserver :1 rendering to
// /dev/fb0, launch an xterm against it, then idle so external screenshotter
// can capture the rendered display.
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

static void start_bg(const char *cmd) {
    pid_t pid = fork();
    if (pid < 0) return;
    if (pid == 0) {
        chroot(ROOT); chdir("/");
        char *envp[] = {
            "PATH=/usr/bin:/usr/sbin:/sbin:/bin",
            "HOME=/root", "TERM=vt100",
            "DISPLAY=:1",
            NULL
        };
        char *argv[] = { "/bin/sh", "-c", (char *)cmd, NULL };
        execve("/bin/sh", argv, envp);
        _exit(127);
    }
}

static int sh_run(const char *cmd, int timeout_ms) {
    pid_t pid = fork();
    if (pid < 0) return -1;
    if (pid == 0) {
        chroot(ROOT); chdir("/");
        char *envp[] = {
            "PATH=/usr/bin:/usr/sbin:/sbin:/bin",
            "HOME=/root", "TERM=vt100",
            "DISPLAY=:1",
            NULL
        };
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
    kill(pid, SIGKILL); waitpid(pid, NULL, 0);
    return -2;
}

int main(void) {
    printf("KX_VISIBLE: start\n"); fflush(stdout);

    mkdir(ROOT, 0755);
    if (mount("/dev/vda1", ROOT, "ext4", MS_RELATIME, NULL) != 0
        && mount("/dev/vda", ROOT, "ext4", MS_RELATIME, NULL) != 0
        && mount("/dev/vda", ROOT, "ext2", MS_RELATIME, NULL) != 0
        && mount("/dev/vda1", ROOT, "ext2", MS_RELATIME, NULL) != 0) {
        printf("KX_VISIBLE: mount failed errno=%d\n", errno); fflush(stdout);
        while (1) sleep(60);
    }
    printf("KX_VISIBLE: rootfs mounted\n"); fflush(stdout);
    mount("/dev", ROOT "/dev", NULL, MS_BIND, NULL);
    mount("/proc", ROOT "/proc", NULL, MS_BIND, NULL);
    mount("/sys", ROOT "/sys", NULL, MS_BIND, NULL);

    // Make sure /tmp/.X11-unix exists inside the chroot (kxserver needs it).
    sh_run("mkdir -p /tmp/.X11-unix; chmod 1777 /tmp/.X11-unix; "
           "rm -f /tmp/.X11-unix/X1 /tmp/.X1-lock", 1000);

    // Start kxserver on :1.
    printf("KX_VISIBLE: starting kxserver :1\n"); fflush(stdout);
    start_bg("/usr/bin/kxserver :1 --log=warn >/tmp/kxserver.log 2>&1");
    sleep(3);
    int rc = sh_run("pgrep -x kxserver >/dev/null 2>&1", 2000);
    printf("KX_VISIBLE: kxserver alive=%d\n", rc == 0); fflush(stdout);

    if (rc != 0) {
        char b[1024]; int p[2]; pipe(p);
        pid_t pid = fork();
        if (pid == 0) {
            close(p[0]); dup2(p[1], 1); dup2(p[1], 2); close(p[1]);
            chroot(ROOT); chdir("/");
            execl("/bin/sh", "sh", "-c", "cat /tmp/kxserver.log 2>&1", NULL);
            _exit(127);
        }
        close(p[1]);
        int n = read(p[0], b, sizeof(b) - 1);
        if (n > 0) { b[n] = 0; printf("  kxserver.log: %s\n", b); }
        close(p[0]); waitpid(pid, NULL, 0);
        fflush(stdout);
    }

    // Paint blue via xsetroot against kxserver.
    rc = sh_run("xsetroot -solid '#2266aa' 2>&1", 3000);
    printf("KX_VISIBLE: xsetroot blue rc=%d\n", rc); fflush(stdout);

    // Launch xterm against kxserver. Using basic bitmap fonts (no Xft).
    start_bg("xterm -geometry 80x24+50+50 "
             "-bg '#001122' -fg '#ffff00' "
             "+ls -title 'Kevlar + kxserver' "
             "-e /bin/sh -c 'clear; echo; "
             "echo \"  ==========================================\"; "
             "echo \"   HELLO FROM KEVLAR + kxserver (Rust X11)\"; "
             "echo \"  ==========================================\"; "
             "echo; echo \"  Rust kernel + Rust X11 server\"; "
             "echo \"  rendering directly to /dev/fb0\"; "
             "echo; hostname; uname -a; echo; "
             "while :; do date; sleep 2; done' 2>/tmp/xterm.err");
    sleep(4);
    rc = sh_run("pgrep -x xterm >/dev/null 2>&1", 2000);
    printf("KX_VISIBLE: xterm alive=%d\n", rc == 0); fflush(stdout);

    printf("KX_VISIBLE: ready for screenshots, sleeping forever\n"); fflush(stdout);
    while (1) sleep(60);
}
