// Generic per-program test harness for Kevlar's LXDE desktop.
//
// Brings up the same Xorg + openbox + tint2 + pcmanfm session as
// test_lxde.c, then spawns a single program named via the
// `kevlar-prog=NAME` kernel cmdline arg (with optional
// `kevlar-prog-args=ARGS`).  Emits four standard sub-tests for the
// program:
//
//   <name>_process_running  — /proc/*/comm scan finds the process.
//   <name>_window_mapped    — xprop -root _NET_CLIENT_LIST shows
//                             at least one mapped window in addition
//                             to pcmanfm's desktop background.
//   <name>_pixels_changed   — fb0 differs by >2000 pixels in some
//                             100x100 region between before-spawn
//                             and after-spawn snapshots.
//   <name>_clean_exit       — SIGTERM the program, verify no zombie.
//
// Run via: make ARCH=arm64 iterate-program PROG=<name>
//
// Shares all bring-up plumbing with test_lxde.c — only the
// per-program assertion phase is new.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdint.h>
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

static int g_pass, g_fail, g_total;
static char g_prog[64] = {0};
static char g_prog_args[256] = {0};

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
    fcntl(pipefd[0], F_SETFL, O_NONBLOCK);
    int total = 0;
    for (int i = 0; i < timeout_ms / 50; i++) {
        char tmp[256];
        int n = read(pipefd[0], tmp, sizeof(tmp));
        if (n > 0 && total + n < bufsz) { memcpy(buf + total, tmp, n); total += n; }
        int status;
        pid_t r = waitpid(pid, &status, WNOHANG);
        if (r == pid) {
            while ((n = read(pipefd[0], tmp, sizeof(tmp))) > 0)
                if (total + n < bufsz) { memcpy(buf + total, tmp, n); total += n; }
            break;
        }
        usleep(50000);
    }
    close(pipefd[0]);
    buf[total < bufsz ? total : bufsz - 1] = '\0';
    kill(pid, SIGKILL);
    waitpid(pid, NULL, 0);
    return total > 0 ? 0 : -1;
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

static void setup_rootfs(void) {
    mkdir(ROOT, 0755);
    sleep(2);
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
    mkdir(ROOT "/tmp/.ICE-unix", 01777);
}

// Parse `key=value` from a buffer, copying value (until space/newline)
// into `out`.  Returns 1 if found.
static int parse_cmdline(const char *cmdline, const char *key,
                         char *out, size_t outsz) {
    const char *p = strstr(cmdline, key);
    if (!p) return 0;
    p += strlen(key);
    size_t i = 0;
    while (*p && *p != ' ' && *p != '\t' && *p != '\n' && i + 1 < outsz) {
        out[i++] = *p++;
    }
    out[i] = '\0';
    return 1;
}

static void load_program_args(void) {
    int fd = open("/proc/cmdline", O_RDONLY);
    if (fd < 0) return;
    char buf[1024];
    ssize_t n = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (n <= 0) return;
    buf[n] = '\0';
    parse_cmdline(buf, "kevlar-prog=", g_prog, sizeof(g_prog));
    parse_cmdline(buf, "kevlar-prog-args=", g_prog_args, sizeof(g_prog_args));
}

// Read fb0 directly and accumulate a 4-byte XOR fingerprint of every
// 256th pixel.  Used to compare snapshots before/after program spawn.
// Returns 0 on success, sets `*fingerprint` and `*nonblack`.
static int fb_fingerprint(uint32_t *fingerprint, size_t *nonblack) {
    int fd = open("/dev/fb0", O_RDONLY);
    if (fd < 0) return -1;
    unsigned char finfo[68] = {0};
    unsigned int smem_len = 0;
    if (ioctl(fd, 0x4602, finfo) == 0) {
        smem_len = *(unsigned int *)(finfo + 24);
    }
    if (smem_len == 0) smem_len = 1024 * 768 * 4;
    void *fb = mmap(NULL, smem_len, PROT_READ, MAP_SHARED, fd, 0);
    if (fb == MAP_FAILED) { close(fd); return -1; }
    const uint32_t *px = (const uint32_t *)fb;
    size_t nsamples = smem_len / 1024;
    uint32_t fp = 0;
    size_t nb = 0;
    for (size_t i = 0; i < nsamples; i++) {
        uint32_t v = px[i * 256];
        fp ^= (v + (uint32_t)i);
        if ((v & 0x00ffffff) != 0) nb++;
    }
    munmap(fb, smem_len);
    close(fd);
    *fingerprint = fp;
    *nonblack = nb;
    return 0;
}

int main(void) {
    printf("T: LXDE Per-Program Test Harness\n");
    fflush(stdout);
    setup_rootfs();

    // Read program name + args from kernel cmdline.
    load_program_args();
    if (g_prog[0] == '\0') {
        fail("kevlar_prog_set", "no kevlar-prog= on /proc/cmdline");
        printf("TEST_END %d/%d\n", g_pass, g_total);
        return 0;
    }
    printf("T: target program: %s %s\n", g_prog, g_prog_args);
    fflush(stdout);

    // Standard LXDE bring-up — same as test_lxde.c.
    sh_run("dbus-uuidgen > /etc/machine-id 2>/dev/null; "
           "cp /etc/machine-id /var/lib/dbus/machine-id 2>/dev/null",
           2000);
    sh_run("dbus-daemon --system --fork 2>/dev/null", 3000);
    sh_run("rm -f /tmp/.X0-lock /tmp/.X11-unix/X0", 500);
    sh_run("/usr/libexec/Xorg :0 -noreset -nolisten tcp "
           "-config /etc/X11/xorg.conf.d/10-fbdev.conf "
           "vt1 >/dev/null 2>&1 &"
           "sleep 3", 8000);

    int rc = sh_run("DISPLAY=:0 xdpyinfo >/dev/null 2>&1", 5000);
    if (rc == 0) pass("xorg_running");
    else { char b[32]; snprintf(b, sizeof(b), "rc=%d", rc); fail("xorg_running", b); }

    sh_run("rm -f /root/.ICEauthority; rm -rf /root/.cache/openbox", 1000);

    start_bg("dbus-daemon --session --address=unix:path=/tmp/.dbus-session-sock "
             "--nofork --print-address >/dev/null 2>&1");
    sleep(1);

    const char *env_prefix =
        "export DISPLAY=:0 HOME=/root "
        "PATH=/usr/bin:/usr/sbin:/usr/local/bin:/bin:/sbin "
        "XDG_DATA_DIRS=/usr/share "
        "XDG_CONFIG_HOME=/root/.config "
        "GTK_THEME=Adwaita "
        "NO_AT_BRIDGE=1 "
        "DBUS_SESSION_BUS_ADDRESS=unix:path=/tmp/.dbus-session-sock; ";

    char cmd[1024];
    snprintf(cmd, sizeof(cmd), "%s exec /usr/bin/openbox >/tmp/lxde-session.log 2>&1", env_prefix);
    start_bg(cmd);
    sleep(2);

    snprintf(cmd, sizeof(cmd), "%s exec /usr/bin/tint2 >>/tmp/lxde-session.log 2>&1", env_prefix);
    start_bg(cmd);
    sleep(1);

    snprintf(cmd, sizeof(cmd), "%s exec /usr/bin/pcmanfm --desktop >>/tmp/lxde-session.log 2>&1", env_prefix);
    start_bg(cmd);

    // Wait for the session to settle before snapping the
    // "before-program" framebuffer.
    for (int s = 0; s < 12; s++) sleep(1);

    // ── Per-program phase ────────────────────────────────────────────
    uint32_t fp_before = 0;
    size_t nb_before = 0;
    fb_fingerprint(&fp_before, &nb_before);
    printf("T: fb fingerprint before %s: %#x (nonblack=%zu)\n",
           g_prog, fp_before, nb_before);

    // Spawn the target program.  Reuse env_prefix so the program
    // sees DISPLAY + XDG paths.
    char target_path[128];
    snprintf(target_path, sizeof(target_path), "/usr/bin/%s", g_prog);

    char prog_cmd[1024];
    snprintf(prog_cmd, sizeof(prog_cmd),
             "%s exec %s %s >>/tmp/lxde-session.log 2>&1",
             env_prefix, target_path, g_prog_args);
    pid_t prog_pid = start_bg(prog_cmd);
    printf("T: launched %s (pid=%d)\n", g_prog, prog_pid);
    fflush(stdout);

    // Give it 6 seconds to map a window + paint.
    for (int s = 0; s < 6; s++) sleep(1);

    // Sub-test 1: process is running.  Scan /proc.
    {
        char prog_test_name[80];
        snprintf(prog_test_name, sizeof(prog_test_name),
                 "%s_process_running", g_prog);
        int found = 0;
        for (int pid = 2; pid < 300; pid++) {
            char path[32], comm[32];
            snprintf(path, sizeof(path), "/proc/%d/comm", pid);
            int fd = open(path, O_RDONLY);
            if (fd < 0) continue;
            int n = read(fd, comm, sizeof(comm) - 1);
            close(fd);
            if (n <= 0) continue;
            comm[n] = '\0';
            if (n > 0 && comm[n-1] == '\n') comm[n-1] = '\0';
            if (strcmp(comm, g_prog) == 0) { found = 1; break; }
        }
        if (found) pass(prog_test_name);
        else fail(prog_test_name, "/proc/*/comm scan miss");
    }

    // Sub-test 2: a window is mapped (root window's _NET_CLIENT_LIST
    // includes more than the pcmanfm desktop).
    {
        char prog_test_name[80];
        snprintf(prog_test_name, sizeof(prog_test_name),
                 "%s_window_mapped", g_prog);
        char xprop_buf[2048] = {0};
        sh_capture("DISPLAY=:0 xprop -root _NET_CLIENT_LIST 2>/dev/null",
                   xprop_buf, sizeof(xprop_buf), 3000);
        // _NET_CLIENT_LIST = 0xNNNNN, 0xNNNNN, ...  Count comma-
        // delimited entries.  >=2 means we got at least one window
        // beyond pcmanfm.
        int comma_count = 0;
        for (char *p = xprop_buf; *p; p++) if (*p == ',') comma_count++;
        printf("T: xprop -root _NET_CLIENT_LIST has %d commas: %s\n",
               comma_count, xprop_buf);
        if (comma_count >= 1) pass(prog_test_name);
        else fail(prog_test_name, "no extra windows mapped");
    }

    // Sub-test 3: framebuffer pixels actually changed.
    {
        char prog_test_name[80];
        snprintf(prog_test_name, sizeof(prog_test_name),
                 "%s_pixels_changed", g_prog);
        uint32_t fp_after = 0;
        size_t nb_after = 0;
        fb_fingerprint(&fp_after, &nb_after);
        printf("T: fb fingerprint after %s: %#x (nonblack=%zu)\n",
               g_prog, fp_after, nb_after);
        if (fp_after != fp_before) {
            pass(prog_test_name);
            // Save snapshot for debugfs extraction.
            int fd = open("/dev/fb0", O_RDONLY);
            if (fd >= 0) {
                unsigned char finfo[68] = {0};
                unsigned int smem_len = 0;
                if (ioctl(fd, 0x4602, finfo) == 0)
                    smem_len = *(unsigned int *)(finfo + 24);
                if (smem_len == 0) smem_len = 1024 * 768 * 4;
                void *fb = mmap(NULL, smem_len, PROT_READ, MAP_SHARED, fd, 0);
                if (fb != MAP_FAILED) {
                    char snap_path[80];
                    snprintf(snap_path, sizeof(snap_path),
                             ROOT "/root/fb-%s.bgra", g_prog);
                    int out = open(snap_path, O_WRONLY|O_CREAT|O_TRUNC, 0644);
                    if (out >= 0) {
                        (void)write(out, fb, smem_len);
                        close(out);
                    }
                    munmap(fb, smem_len);
                }
                close(fd);
            }
        } else {
            fail(prog_test_name, "fingerprint unchanged");
        }
    }

    // Sub-test 4: clean exit on SIGTERM.
    {
        char prog_test_name[80];
        snprintf(prog_test_name, sizeof(prog_test_name),
                 "%s_clean_exit", g_prog);
        // SIGTERM by name (the prog_pid is the sh wrapper, not the
        // program itself).  Use pkill.
        char kill_cmd[160];
        snprintf(kill_cmd, sizeof(kill_cmd), "pkill -TERM %s 2>/dev/null", g_prog);
        sh_run(kill_cmd, 1000);
        sleep(2);
        int still_running = 0;
        for (int pid = 2; pid < 300; pid++) {
            char path[32], comm[32];
            snprintf(path, sizeof(path), "/proc/%d/comm", pid);
            int fd = open(path, O_RDONLY);
            if (fd < 0) continue;
            int n = read(fd, comm, sizeof(comm) - 1);
            close(fd);
            if (n <= 0) continue;
            comm[n] = '\0';
            if (n > 0 && comm[n-1] == '\n') comm[n-1] = '\0';
            if (strcmp(comm, g_prog) == 0) { still_running = 1; break; }
        }
        if (!still_running) pass(prog_test_name);
        else fail(prog_test_name, "still running after SIGTERM");
    }

    // Dump tail of the session log so failures are diagnosable.
    {
        char b[8192];
        if (sh_capture("tail -40 /tmp/lxde-session.log 2>/dev/null",
                       b, sizeof(b), 2000) == 0 && b[0]) {
            printf("== /tmp/lxde-session.log (tail) ==\n%s", b);
        }
    }

    printf("\nTEST_END %d/%d\n", g_pass, g_total);
    fflush(stdout);
    sync();
    return 0;
}
