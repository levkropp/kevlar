// Diagnostic variant of test_xfce that focuses on xfce4-panel startup.
//
// Baseline test-xfce consistently fails xfce4_panel_running even when xfwm4
// and xfce4-session come up cleanly. This probe extends the observation
// window to 60 s and dumps /proc state per-second so we can see:
//   - does a panel process ever get spawned?
//   - if yes, what's it doing (syscall, status)?
//   - if no, what does xfce4-session's log say?
//
// Run via: make build INIT_SCRIPT=/bin/test-xfce-panel-probe
#define _GNU_SOURCE
#include <dirent.h>
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
#include <sched.h>
#include <unistd.h>

#define ROOT "/mnt"

static int sh_run(const char *cmd, int timeout_ms) {
    pid_t pid = fork();
    if (pid < 0) return -1;
    if (pid == 0) {
        chroot(ROOT);
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

static pid_t start_bg(const char *cmd) {
    pid_t pid = fork();
    if (pid == 0) {
        if (chroot(ROOT) != 0) _exit(126);
        chdir("/");
        setsid();
        int fd = open("/dev/null", O_RDWR);
        if (fd >= 0) { dup2(fd, 0); if (fd > 2) close(fd); }
        execl("/bin/sh", "sh", "-c", cmd, NULL);
        _exit(127);
    }
    return pid;
}

struct procinfo {
    int panel_pid;
    int session_pid;
    int wm_pid;
    int settings_pid;
    int conf_pid;
    int proc_count;
};

static struct procinfo scan_procs(void) {
    struct procinfo r = { -1, -1, -1, -1, -1, 0 };
    DIR *d = opendir("/proc");
    if (!d) return r;
    struct dirent *de;
    while ((de = readdir(d)) != NULL) {
        if (de->d_name[0] < '0' || de->d_name[0] > '9') continue;
        r.proc_count++;
        char path[64], comm[32];
        snprintf(path, sizeof(path), "/proc/%s/comm", de->d_name);
        int fd = open(path, O_RDONLY);
        if (fd < 0) continue;
        int n = read(fd, comm, sizeof(comm) - 1);
        close(fd);
        if (n <= 0) continue;
        if (comm[n-1] == '\n') comm[n-1] = 0;
        else comm[n] = 0;
        int pid = atoi(de->d_name);
        if (strcmp(comm, "xfce4-panel") == 0) r.panel_pid = pid;
        else if (strcmp(comm, "xfce4-session") == 0) r.session_pid = pid;
        else if (strcmp(comm, "xfwm4") == 0) r.wm_pid = pid;
        else if (strcmp(comm, "xfsettingsd") == 0) r.settings_pid = pid;
        else if (strcmp(comm, "xfconfd") == 0) r.conf_pid = pid;
    }
    closedir(d);
    return r;
}

static void setup_rootfs(void) {
    mkdir(ROOT, 0755);
    sleep(2);
    if (mount("/dev/vda", ROOT, "ext2", 0, NULL) != 0) {
        printf("MOUNT FAILED errno=%d\n", errno);
        _exit(1);
    }
    mkdir(ROOT "/proc", 0755);  mount("proc",     ROOT "/proc", "proc",     0, NULL);
    mkdir(ROOT "/sys", 0755);   mount("sysfs",    ROOT "/sys",  "sysfs",    0, NULL);
    mkdir(ROOT "/dev", 0755);   mount("devtmpfs", ROOT "/dev",  "devtmpfs", 0, NULL);
    mkdir(ROOT "/dev/pts", 0755);
    mkdir(ROOT "/dev/shm", 0755);
    mkdir(ROOT "/dev/input", 0755);
    mkdir(ROOT "/tmp", 01777);  mount("tmpfs",    ROOT "/tmp",  "tmpfs",    0, NULL);
    mkdir(ROOT "/run", 0755);   mount("tmpfs",    ROOT "/run",  "tmpfs",    0, NULL);
    mkdir(ROOT "/run/dbus", 0755);
    mkdir(ROOT "/tmp/.X11-unix", 01777);
    mkdir(ROOT "/tmp/.ICE-unix", 01777);
}

int main(void) {
    printf("PANEL_PROBE: start\n"); fflush(stdout);
    setup_rootfs();

    // Bring up the same support as Phase 5 of test-xfce.
    sh_run("dbus-uuidgen > /etc/machine-id 2>/dev/null; "
           "cp /etc/machine-id /var/lib/dbus/machine-id 2>/dev/null", 2000);
    sh_run("dbus-daemon --system --fork 2>/dev/null", 3000);
    sh_run("rm -f /tmp/.X0-lock /tmp/.X11-unix/X0", 500);
    sh_run("/usr/libexec/Xorg :0 -noreset -nolisten tcp "
           "-config /etc/X11/xorg.conf.d/10-fbdev.conf "
           "vt1 >/tmp/xorg.log 2>&1 &"
           "sleep 3", 8000);

    sh_run("rm -f /root/.ICEauthority; "
           "rm -rf /root/.config/xfce4/xfconf/xfce-perchannel-xml/", 1000);
    sh_run("mkdir -p /root/.config/xfce4/xfconf/xfce-perchannel-xml && "
           "cat > /root/.config/xfce4/xfconf/xfce-perchannel-xml/xfwm4.xml << 'X'\n"
           "<?xml version=\"1.0\"?>\n"
           "<channel name=\"xfwm4\"><property name=\"general\" type=\"empty\">\n"
           "<property name=\"use_compositing\" type=\"bool\" value=\"false\"/>\n"
           "</property></channel>\n"
           "X", 3000);

    // Launch dbus-session + xfce4-session the same way test-xfce does.
    start_bg("dbus-daemon --session "
             "--address=unix:path=/tmp/.dbus-session-sock "
             "--nofork --print-address >/dev/null 2>&1");
    sleep(1);
    start_bg("export DISPLAY=:0 HOME=/root "
             "PATH=/usr/bin:/usr/sbin:/usr/local/bin:/bin:/sbin "
             "XDG_DATA_DIRS=/usr/share XDG_CONFIG_HOME=/root/.config "
             "GTK_THEME=Adwaita NO_AT_BRIDGE=1 "
             "DBUS_SESSION_BUS_ADDRESS=unix:path=/tmp/.dbus-session-sock; "
             "exec /usr/bin/xfce4-session >/tmp/xfce-session.log 2>&1");

    // Probe every second for 120 s. If a client takes ~25s per registration
    // timeout, the panel might appear around T+75-T+100.
    int panel_first_seen = -1;
    for (int t = 0; t < 120; t++) {
        sleep(1);
        struct procinfo r = scan_procs();
        if (r.panel_pid > 0 && panel_first_seen < 0) {
            panel_first_seen = t + 1;
        }
        // Only print every 2 s after T+10 to keep log shorter.
        if (t < 10 || (t % 2) == 0 || r.panel_pid > 0) {
            printf("  T+%-3d nproc=%d session=%d wm=%d set=%d conf=%d panel=%d\n",
                   t + 1, r.proc_count, r.session_pid, r.wm_pid,
                   r.settings_pid, r.conf_pid, r.panel_pid);
            fflush(stdout);
        }
        if (r.panel_pid > 0 && t > panel_first_seen + 5) break;
    }
    printf("\nSUMMARY panel_first_seen=T+%d\n", panel_first_seen);

    // Final dump of xfce-session log (tells us what xfce4-session attempted
    // to spawn and whether any Failsafe-client spawn errored). The log is
    // in the chroot's /tmp, which is /mnt/tmp from outside.
    printf("\n=== /mnt/tmp/xfce-session.log ===\n"); fflush(stdout);
    {
        char b[8192];
        int fd = open("/mnt/tmp/xfce-session.log", O_RDONLY);
        if (fd >= 0) {
            int n = read(fd, b, sizeof(b) - 1);
            if (n > 0) { b[n] = 0; printf("%s\n", b); }
            close(fd);
        } else {
            printf("(open failed errno=%d)\n", errno);
        }
    }

    printf("PANEL_PROBE: done\n"); fflush(stdout);
    sync();
    _exit(0);
}
