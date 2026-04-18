// Launch XFCE components DIRECTLY (bypass xfce4-session which SIGSEGVs
// intermittently) so we can screenshot a real XFCE desktop.
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
        chroot(ROOT); chdir("/");
        char *envp[] = {
            "PATH=/usr/bin:/usr/sbin:/sbin:/bin",
            "HOME=/root", "TERM=vt100",
            "DISPLAY=:0",
            "DBUS_SESSION_BUS_ADDRESS=unix:path=/tmp/.dbus-session-sock",
            "XDG_DATA_DIRS=/usr/share",
            "XDG_CONFIG_HOME=/root/.config",
            "GTK_THEME=Adwaita",
            "NO_AT_BRIDGE=1",
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

static void start_bg(const char *cmd) {
    pid_t pid = fork();
    if (pid < 0) return;
    if (pid == 0) {
        chroot(ROOT); chdir("/");
        char *envp[] = {
            "PATH=/usr/bin:/usr/sbin:/sbin:/bin",
            "HOME=/root", "TERM=vt100",
            "DISPLAY=:0",
            "DBUS_SESSION_BUS_ADDRESS=unix:path=/tmp/.dbus-session-sock",
            "XDG_DATA_DIRS=/usr/share",
            "XDG_CONFIG_HOME=/root/.config",
            "GTK_THEME=Adwaita",
            "NO_AT_BRIDGE=1",
            NULL
        };
        char *argv[] = { "/bin/sh", "-c", (char *)cmd, NULL };
        execve("/bin/sh", argv, envp);
        _exit(127);
    }
}

int main(void) {
    printf("XFCE_DIRECT: start\n"); fflush(stdout);

    mkdir(ROOT, 0755);
    if (mount("/dev/vda1", ROOT, "ext4", MS_RELATIME, NULL) != 0
        && mount("/dev/vda", ROOT, "ext4", MS_RELATIME, NULL) != 0) {
        printf("XFCE_DIRECT: mount failed errno=%d\n", errno); fflush(stdout);
        while (1) sleep(60);
    }
    printf("XFCE_DIRECT: rootfs mounted\n"); fflush(stdout);
    mount("/dev", ROOT "/dev", NULL, MS_BIND, NULL);
    mount("/proc", ROOT "/proc", NULL, MS_BIND, NULL);
    mount("/sys", ROOT "/sys", NULL, MS_BIND, NULL);

    // Generate machine-id and font dirs.
    sh_run("dbus-uuidgen > /etc/machine-id 2>/dev/null; "
           "cp /etc/machine-id /var/lib/dbus/machine-id 2>/dev/null", 2000);
    sh_run("mkfontscale /usr/share/fonts/misc 2>/dev/null; "
           "mkfontdir /usr/share/fonts/misc 2>/dev/null", 10000);

    // Clean stale X lock + D-Bus.
    sh_run("rm -f /tmp/.X0-lock /tmp/.X11-unix/X0 /run/dbus/dbus.pid "
           "/tmp/.dbus-session-sock", 1000);

    // D-Bus: system bus (forked) + session bus (background).
    sh_run("dbus-daemon --system --fork 2>/dev/null", 3000);
    start_bg("dbus-daemon --session "
             "--address=unix:path=/tmp/.dbus-session-sock "
             "--nofork 2>/dev/null");
    sleep(1);

    // Xorg.
    printf("XFCE_DIRECT: starting Xorg\n"); fflush(stdout);
    start_bg("/usr/libexec/Xorg :0 -noreset -nolisten tcp "
             "-config /etc/X11/xorg.conf.d/10-fbdev.conf "
             "vt1 >/tmp/xorg.log 2>&1");
    sleep(4);
    int rc = sh_run("pgrep -x Xorg >/dev/null 2>&1", 2000);
    printf("XFCE_DIRECT: Xorg alive=%d\n", rc == 0); fflush(stdout);

    // Paint solid background (fallback in case xfdesktop fails).
    sh_run("xsetroot -solid '#1a1a2e' 2>/dev/null", 5000);

    // XFCE components, one at a time with delay.
    printf("XFCE_DIRECT: starting xfsettingsd\n"); fflush(stdout);
    start_bg("xfsettingsd 2>/tmp/xfsettingsd.log");
    sleep(1);

    printf("XFCE_DIRECT: starting xfwm4\n"); fflush(stdout);
    start_bg("xfwm4 --display :0.0 --replace 2>/tmp/xfwm4.log");
    sleep(2);

    printf("XFCE_DIRECT: starting xfdesktop\n"); fflush(stdout);
    start_bg("xfdesktop --display :0.0 --disable-wm-check 2>/tmp/xfdesktop.log");
    sleep(3);

    printf("XFCE_DIRECT: starting xfce4-panel\n"); fflush(stdout);
    start_bg("xfce4-panel --display :0.0 --disable-wm-check 2>/tmp/xfce4-panel.log");
    sleep(3);

    // Report running processes.
    {
        char b[1024]; int p[2]; pipe(p);
        pid_t pid = fork();
        if (pid == 0) {
            close(p[0]); dup2(p[1], 1); dup2(p[1], 2); close(p[1]);
            chroot(ROOT); chdir("/");
            execl("/bin/sh", "sh", "-c",
                  "ps -e -o pid,comm 2>/dev/null | "
                  "grep -E 'Xorg|xfwm4|xfce4|xfsettings|xfdesktop|dbus'",
                  NULL);
            _exit(127);
        }
        close(p[1]);
        int n = read(p[0], b, sizeof(b) - 1);
        if (n > 0) { b[n] = 0; printf("XFCE_DIRECT processes:\n%s", b); }
        close(p[0]); waitpid(pid, NULL, 0);
        fflush(stdout);
    }

    printf("XFCE_DIRECT: ready for screenshots, sleeping forever\n");
    fflush(stdout);
    while (1) sleep(60);
}
