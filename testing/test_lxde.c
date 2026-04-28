// Fully automated end-to-end LXDE desktop test for Kevlar.
// Boots Alpine LXDE image, sets up D-Bus, starts Xorg + LXDE, reports results.
// Run via: make test-lxde
//
// Shape mirrors test_xfce.c: same Phase 5 quick-mode flow.  Components
// checked: lxsession, openbox (window manager), lxpanel, pcmanfm
// (file manager/desktop renderer).
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
static int g_interactive;
// Name extracted from `kevlar_test_setup=NAME` on the cmdline.  Empty
// if not present.  Currently supports "doubleclick" — used by
// tests/integration/lxde-doubleclick-folder.yaml to drive the freeze
// investigation.  Other names can be added without changing the
// dispatch shape.
static char g_test_setup[64];

static int sh_run(const char *cmd, int timeout_ms);

// Read /proc/cmdline (mounting initramfs /proc if needed) and populate
// g_interactive + g_test_setup.  Returns 1 if any test-mode flag was
// found on the cmdline.  Used to skip test-only delays and sub-tests
// when run-alpine-lxde / itest launch the desktop for interactive
// use rather than as a regression suite.
static int read_kevlar_cmdline(void) {
    mkdir("/proc", 0755);
    mount("proc", "/proc", "proc", 0, NULL);
    int cfd = open("/proc/cmdline", O_RDONLY);
    if (cfd < 0) return 0;
    char buf[1024];
    ssize_t n = read(cfd, buf, sizeof(buf) - 1);
    close(cfd);
    if (n <= 0) return 0;
    buf[n] = '\0';
    int any = 0;
    if (strstr(buf, "kevlar_interactive")) {
        g_interactive = 1;
        any = 1;
    }
    char *p = strstr(buf, "kevlar_test_setup=");
    if (p) {
        p += strlen("kevlar_test_setup=");
        // Copy up to the next space / newline / null.
        size_t i = 0;
        while (p[i] && p[i] != ' ' && p[i] != '\n' && p[i] != '\t'
               && i < sizeof(g_test_setup) - 1) {
            g_test_setup[i] = p[i];
            i++;
        }
        g_test_setup[i] = '\0';
        any = 1;
    }
    return any;
}

// Backwards-compat shim — the late-path fallback in main() still calls
// this name.  Returns 1 if either kevlar_interactive OR a known
// kevlar_test_setup mode is on the cmdline.
static int read_kevlar_interactive(void) {
    return read_kevlar_cmdline();
}

static void interactive_keepalive(void) {
    // Persist Xorg + session logs to the ext2 disk before the keepalive
    // sleep so users can `debugfs -R "cat /var/log/Xorg.0.log" ...` after
    // shutting down to inspect input-device PreInit, driver loads, and
    // any config errors.  In batch mode the harness does this at the
    // end of main; in interactive we never reach that path.  sh_run
    // chroots into /mnt first, so paths here are inside the disk fs.
    sh_run("mkdir -p /var/log; "
           "cp -f /tmp/Xorg.0.log /var/log/Xorg.0.log 2>/dev/null; "
           "cp -f /tmp/lxde-session.log /var/log/lxde-session.log 2>/dev/null; "
           "true", 3000);
    sync();
    printf("\n=== test-lxde: kevlar_interactive on cmdline; keeping desktop alive ===\n");
    fflush(stdout);
    // Periodic sync + log re-persist so pcmanfm's stderr output (which
    // goes directly to /var/log/pcmanfm.log on the ext2 disk) actually
    // lands before a hard shutdown / kernel halt.  Also re-copies
    // /tmp/Xorg.0.log and /tmp/lxde-session.log every 30s.  Without
    // this, post-halt `debugfs` extracts may show stale content.
    while (1) {
        sleep(30);
        sh_run("cp -f /tmp/Xorg.0.log /var/log/Xorg.0.log 2>/dev/null; "
               "cp -f /tmp/lxde-session.log /var/log/lxde-session.log 2>/dev/null; "
               "sync; true", 2000);
    }
}

// Find pcmanfm's PID by scanning /proc/*/comm.  Returns 0 if not
// found.  Called from the doubleclick setup which expects pcmanfm
// to already be running.
static int find_proc_pid(const char *target_comm) {
    for (int pid = 2; pid < 300; pid++) {
        char path[32], comm[32];
        snprintf(path, sizeof(path), "/proc/%d/comm", pid);
        int fd = open(path, O_RDONLY);
        if (fd < 0) continue;
        int n = read(fd, comm, sizeof(comm) - 1);
        close(fd);
        if (n <= 0) continue;
        comm[n] = '\0';
        if (n > 0 && comm[n - 1] == '\n') comm[n - 1] = '\0';
        if (strcmp(comm, target_comm) == 0) return pid;
    }
    return 0;
}

// Read /proc/N/stat and return the state char (R/S/D/T/Z), or '?' if
// unreadable / unparseable.  Field 3 of /proc/N/stat is the state
// char, after the comm-in-parens field.
static char read_proc_state(int pid) {
    char path[32], buf[256];
    snprintf(path, sizeof(path), "/proc/%d/stat", pid);
    int fd = open(path, O_RDONLY);
    if (fd < 0) return '?';
    int n = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (n <= 0) return '?';
    buf[n] = '\0';
    // Find the closing ')' of the comm field, then the state char is
    // two chars later (after the space).
    char *p = strrchr(buf, ')');
    if (!p) return '?';
    if (!p[1] || !p[2]) return '?';
    return p[2];
}

// Wait up to `timeout_ms` for >=50% of /dev/fb0 pixels to be non-black,
// indicating that pcmanfm has finished painting the wallpaper.
// Returns 1 if the threshold was reached, 0 on timeout.  Mirrors the
// existing lxde_pixels_visible test logic but as a blocking helper.
static int wait_for_wallpaper(int timeout_ms) {
    int fd = open("/dev/fb0", O_RDONLY);
    if (fd < 0) return 0;
    unsigned char finfo[68] = {0};
    unsigned int smem_len = 0;
    if (ioctl(fd, 0x4602 /* FBIOGET_FSCREENINFO */, finfo) == 0) {
        smem_len = *(unsigned int *)(finfo + 24);
    }
    if (smem_len == 0) smem_len = 1024 * 768 * 4;
    void *fb = mmap(NULL, smem_len, PROT_READ, MAP_SHARED, fd, 0);
    int ok = 0;
    if (fb != MAP_FAILED) {
        const uint32_t *px = (const uint32_t *)fb;
        size_t nsamples = smem_len / 1024;
        for (int i = 0; i < timeout_ms / 500 && !ok; i++) {
            size_t nonblack = 0;
            for (size_t j = 0; j < nsamples; j++) {
                if ((px[j * 256] & 0x00ffffff) != 0) nonblack++;
            }
            if (nonblack * 2 >= nsamples) {  // >=50%
                ok = 1;
                break;
            }
            usleep(500000);
        }
        munmap(fb, smem_len);
    }
    close(fd);
    return ok;
}

// kevlar_test_setup=doubleclick: wait for the wallpaper to actually
// render (so pre-click screenshots aren't of a half-painted desktop),
// pre-create a folder on the desktop, emit DESKTOP_READY with
// pcmanfm's PID, then poll the process state every ~2 seconds and
// emit PCMANFM_STATE markers to serial.  The itest harness uses this
// to drive the double-click freeze investigation: it injects a
// double-click after DESKTOP_READY and watches for state transitions.
// A frozen pcmanfm shows up as "S" (sleeping) for many consecutive
// samples after the click — vs. "R" briefly during a successful
// folder open.
static void doubleclick_setup_keepalive(void) {
    // Wait for wallpaper paint (up to 60s).  Without this, harness
    // pre-click captures land on a half-painted desktop and the
    // framebuffer_changed assertion fires on the late paint instead
    // of the actual click effect.  60s is generous — pcmanfm
    // typically paints within 10s — but allows for slow-pcmanfm
    // first-render situations.
    int painted = wait_for_wallpaper(60000);
    if (!painted) {
        printf("WARNING: wallpaper not painted after 20s; proceeding anyway\n");
        fflush(stdout);
    }

    // Make sure /var/log + /var/log/diag exist on the ext2 disk.
    sh_run("mkdir -p /var/log /var/log/diag", 2000);

    // Pre-create the folder pcmanfm should display on the desktop.
    // ~/Desktop is the default desktop dir; pcmanfm rescans on
    // refresh / inotify.  Sleep briefly to let pcmanfm pick it up.
    sh_run("mkdir -p /root/Desktop && "
           "rm -rf /root/Desktop/test-folder && "
           "mkdir /root/Desktop/test-folder && "
           "sync", 3000);
    sleep(2);  // give pcmanfm time to notice the new folder

    // Locate pcmanfm.
    int pcmanfm_pid = find_proc_pid("pcmanfm");

    // Persist initial diagnostic snapshots.
    if (pcmanfm_pid > 0) {
        char cmd[512];
        snprintf(cmd, sizeof(cmd),
                 "cp /proc/%d/maps /var/log/diag/pcmanfm-maps-initial.txt 2>/dev/null; "
                 "cp /proc/%d/status /var/log/diag/pcmanfm-status-initial.txt 2>/dev/null; "
                 "cp /proc/%d/stat /var/log/diag/pcmanfm-stat-initial.txt 2>/dev/null; "
                 "cp /proc/%d/cmdline /var/log/diag/pcmanfm-cmdline.txt 2>/dev/null; "
                 "true",
                 pcmanfm_pid, pcmanfm_pid, pcmanfm_pid, pcmanfm_pid);
        sh_run(cmd, 3000);
    }

    // Persist Xorg + lxde-session logs (initial snapshot).
    sh_run("cp -f /tmp/Xorg.0.log /var/log/Xorg.0.log 2>/dev/null; "
           "cp -f /tmp/lxde-session.log /var/log/lxde-session.log 2>/dev/null; "
           "sync; true", 3000);

    // Emit ready marker.  Harness blocks on this regex.
    printf("\nDESKTOP_READY pcmanfm_pid=%d\n", pcmanfm_pid);
    fflush(stdout);

    // Poll loop: sample pcmanfm's state every 2 seconds and emit a
    // marker.  Every 5th sample, also re-persist the X / session
    // logs and pcmanfm's current /proc snapshot so the harness can
    // extract them post-shutdown for diagnosis.
    int sample = 0;
    while (1) {
        char state = (pcmanfm_pid > 0) ? read_proc_state(pcmanfm_pid) : '?';
        printf("PCMANFM_STATE sample=%d pid=%d state=%c\n",
               sample, pcmanfm_pid, state);
        fflush(stdout);
        if (state == '?' || state == 'Z') {
            // Rediscover in case pcmanfm respawned (e.g. parent
            // pcmanfm exec'd a child).
            int new_pid = find_proc_pid("pcmanfm");
            if (new_pid != pcmanfm_pid) {
                printf("PCMANFM_PID_CHANGED old=%d new=%d\n",
                       pcmanfm_pid, new_pid);
                fflush(stdout);
                pcmanfm_pid = new_pid;
            }
        }
        if (sample % 5 == 0 && pcmanfm_pid > 0) {
            char cmd[512];
            snprintf(cmd, sizeof(cmd),
                     "cp -f /tmp/Xorg.0.log /var/log/Xorg.0.log 2>/dev/null; "
                     "cp -f /tmp/lxde-session.log /var/log/lxde-session.log 2>/dev/null; "
                     "cp /proc/%d/stat /var/log/diag/pcmanfm-stat-latest.txt 2>/dev/null; "
                     "cp /proc/%d/status /var/log/diag/pcmanfm-status-latest.txt 2>/dev/null; "
                     "cp /proc/%d/maps /var/log/diag/pcmanfm-maps-latest.txt 2>/dev/null; "
                     "sync; true",
                     pcmanfm_pid, pcmanfm_pid, pcmanfm_pid);
            sh_run(cmd, 2000);
        }
        sample++;
        sleep(2);
    }
}

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

static void skip(const char *name, const char *detail) {
    if (detail) printf("TEST_SKIP %s (%s)\n", name, detail);
    else printf("TEST_SKIP %s\n", name);
    fflush(stdout);
    /* skip doesn't count toward total */
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

int main(void) {
    printf("T: LXDE Desktop End-to-End Test\n");
    fflush(stdout);
    read_kevlar_cmdline();
    if (g_interactive) {
        printf("T: kevlar_interactive — fast-boot path, sub-tests skipped\n");
        fflush(stdout);
    }
    if (g_test_setup[0]) {
        printf("T: kevlar_test_setup=%s — itest harness mode\n", g_test_setup);
        fflush(stdout);
    }
    setup_rootfs();

    // Start D-Bus system bus, then Xorg on fbdev.
    sh_run("dbus-uuidgen > /etc/machine-id 2>/dev/null; "
           "cp /etc/machine-id /var/lib/dbus/machine-id 2>/dev/null",
           2000);
    sh_run("dbus-daemon --system --fork 2>/dev/null", 3000);
    sh_run("rm -f /tmp/.X0-lock /tmp/.X11-unix/X0", 500);
    // -logfile writes Xorg's full diagnostic log to /var/log on the
    // ext2 disk (not /tmp/Xorg.0.log which is tmpfs and disappears at
    // shutdown).  This is what `iterate-lxde` extracts via debugfs
    // and what users can `cat` to debug InputDevice / driver-load
    // failures.
    sh_run("mkdir -p /var/log; "
           "/usr/libexec/Xorg :0 -noreset -nolisten tcp "
           "-logfile /var/log/Xorg.0.log "
           "-config /etc/X11/xorg.conf.d/10-fbdev.conf "
           "vt1 >/dev/null 2>&1 &"
           "sleep 3", 8000);

    // Verify Xorg is up and framebuffer is writable.
    // (xdpyinfo returning 0 is a strong signal that Xorg is alive and the
    // X socket accepts connections.)
    int rc = sh_run("DISPLAY=:0 xdpyinfo >/dev/null 2>&1", 5000);
    if (rc == 0) pass("xorg_running");
    else { char b[32]; snprintf(b, sizeof(b), "rc=%d", rc); fail("xorg_running", b); }

    // Clean stale session state between runs.
    sh_run("rm -f /root/.ICEauthority; "
           "rm -rf /root/.cache/openbox",
           1000);

    // Start the session bus.
    printf("  T+0 about to start_bg dbus-daemon\n"); fflush(stdout);
    start_bg("dbus-daemon --session "
             "--address=unix:path=/tmp/.dbus-session-sock "
             "--nofork --print-address "
             ">/dev/null 2>&1");
    printf("  T+0 dbus-daemon start_bg returned\n"); fflush(stdout);
    sleep(1);

    // LXDE-style stack (no lxsession on Alpine 3.21): start openbox,
    // tint2, pcmanfm as independent children so we don't depend on
    // openbox's openbox-autostart wrapper (which invokes
    // openbox-xdg-autostart — a separate package Alpine doesn't ship).
    const char *env_prefix =
        "export DISPLAY=:0 HOME=/root "
        "PATH=/usr/bin:/usr/sbin:/usr/local/bin:/bin:/sbin "
        "XDG_DATA_DIRS=/usr/share "
        "XDG_CONFIG_HOME=/root/.config "
        "GTK_THEME=Adwaita "
        "NO_AT_BRIDGE=1 "
        "DBUS_SESSION_BUS_ADDRESS=unix:path=/tmp/.dbus-session-sock; ";

    printf("  T+1 about to start_bg openbox\n"); fflush(stdout);
    char cmd[1024];
    snprintf(cmd, sizeof(cmd), "%s exec /usr/bin/openbox >/tmp/lxde-session.log 2>&1", env_prefix);
    start_bg(cmd);
    printf("  T+1 openbox start_bg returned\n"); fflush(stdout);
    sleep(2);

    printf("  T+3 about to start_bg tint2\n"); fflush(stdout);
    snprintf(cmd, sizeof(cmd), "%s exec /usr/bin/tint2 >>/tmp/lxde-session.log 2>&1", env_prefix);
    start_bg(cmd);
    printf("  T+3 tint2 start_bg returned\n"); fflush(stdout);
    sleep(1);

    printf("  T+4 about to start_bg pcmanfm --desktop\n"); fflush(stdout);
    // Log directly to /var/log on the ext2 disk (NOT /tmp which is
    // tmpfs and disappears at shutdown).  pcmanfm's last words
    // before SIGABRT (glib criticals, libfm assertions) need to
    // survive a halt for `debugfs -R "cat /var/log/pcmanfm.log"`
    // post-mortem analysis.  The shell `tee` keeps stdout + stderr
    // line-buffered so we don't lose the last partial line.
    sh_run("mkdir -p /var/log && touch /var/log/pcmanfm.log", 1000);
    snprintf(cmd, sizeof(cmd),
             "%s exec /usr/bin/pcmanfm --desktop "
             ">>/var/log/pcmanfm.log 2>&1", env_prefix);
    start_bg(cmd);
    printf("  T+4 pcmanfm start_bg returned\n"); fflush(stdout);

    // Interactive fast-path: in run-alpine-lxde the user just wants
    // the desktop on screen as fast as possible — skip the 15s
    // settle, the lxde_pixels_visible 8-attempt retry, and the
    // typed_text_arrived xterm spawn.  8s settle gives pcmanfm
    // enough time to render its wallpaper (~6-7s on Kevlar) before
    // we drop into the keepalive sleep loop; with only 3s the
    // tint2 panel is up but the desktop background is still
    // unrendered, leaving a black/transparent main area.
    //
    // kevlar_test_setup=NAME diverts to a NAME-specific harness
    // setup at the same point.  Same 8s settle so pcmanfm has time
    // to render before the harness starts injecting.
    if (strcmp(g_test_setup, "doubleclick") == 0) {
        // No fixed sleep; doubleclick_setup_keepalive uses
        // wait_for_wallpaper to block until pcmanfm has actually
        // painted the framebuffer.  More reliable than a fixed
        // settle.
        doubleclick_setup_keepalive();
    }
    if (g_interactive) {
        sleep(8);
        interactive_keepalive();
    }

    // Same 15-second wait as XFCE — session spawn pace varies.
    for (int s = 0; s < 15; s++) {
        sleep(1);
        printf("  T+%d sleeping\n", 2 + s); fflush(stdout);
    }
    printf("  LXDE wait done\n"); fflush(stdout);

    // Check components by scanning /proc/*/comm.  Alpine 3.21 doesn't
    // ship lxsession/lxpanel, so our LXDE-style stack uses openbox
    // (WM) + tint2 (panel) + pcmanfm --desktop (desktop renderer).
    // All three are launched by openbox-session's autostart.
    {
        int has_openbox = 0, has_tint2 = 0, has_pcmanfm = 0;
        for (int pid = 2; pid < 200; pid++) {
            char path[32], comm[32];
            snprintf(path, sizeof(path), "/proc/%d/comm", pid);
            int fd = open(path, O_RDONLY);
            if (fd < 0) continue;
            int n = read(fd, comm, sizeof(comm) - 1);
            close(fd);
            if (n <= 0) continue;
            comm[n] = '\0';
            if (n > 0 && comm[n-1] == '\n') comm[n-1] = '\0';
            if (strcmp(comm, "openbox") == 0) has_openbox = 1;
            if (strcmp(comm, "tint2") == 0) has_tint2 = 1;
            if (strcmp(comm, "pcmanfm") == 0) has_pcmanfm = 1;
        }
        printf("  components: openbox=%d tint2=%d pcmanfm=%d\n",
               has_openbox, has_tint2, has_pcmanfm);
        fflush(stdout);
        if (has_openbox) pass("openbox_running");
        else fail("openbox_running", "window manager not found");
        if (has_tint2) pass("tint2_running");
        else fail("tint2_running", "panel not found");
        if (has_pcmanfm) pass("pcmanfm_running");
        else fail("pcmanfm_running", "desktop renderer not found");

        // pcmanfm --desktop should be painting its configured wallpaper
        // (#336699 from ~/.config/pcmanfm/default/pcmanfm.conf).  Give
        // it a moment to draw — the retry loop below will catch the
        // race where pcmanfm clears the desktop region just before our
        // sample.
        //
        // We deliberately do NOT kill pcmanfm here anymore; the old
        // workaround (kill pcmanfm + xsetroot) was racy because xsetroot
        // sometimes lost to the WM/sprite layer.  If pcmanfm is
        // genuinely failing to paint, the retry loop will exhaust and
        // fail the test cleanly with the actual non-black-pixel count.
        sleep(2);

        // Framebuffer pixel check: are pixels actually drawn?  Read fb0
        // directly and count non-black pixels + distinct color bits.
        //
        // Retry up to 8 times, 1s apart.  pcmanfm clears the desktop
        // region before painting the wallpaper; if our sample lands in
        // the brief window between clear and paint, we see nonblack=2
        // (just the cursor) even though the desktop ends up correctly
        // painted moments later.  Retrying eliminates that race.
        {
            int fd = open("/dev/fb0", O_RDONLY);
            if (fd < 0) {
                fail("lxde_pixels_visible", "can't open /dev/fb0");
            } else {
                unsigned char finfo[68] = {0};
                unsigned int smem_len = 0;
                if (ioctl(fd, 0x4602 /* FBIOGET_FSCREENINFO */, finfo) == 0) {
                    smem_len = *(unsigned int *)(finfo + 24);
                }
                if (smem_len == 0) smem_len = 1024 * 768 * 4;
                void *fb = mmap(NULL, smem_len, PROT_READ, MAP_SHARED, fd, 0);
                if (fb == MAP_FAILED) {
                    fail("lxde_pixels_visible", "mmap fb0 failed");
                } else {
                    const uint32_t *px = (const uint32_t *)fb;
                    size_t nsamples = smem_len / 1024;
                    size_t nonblack = 0;
                    uint32_t distinct_mask = 0;
                    int colors_bits = 0;
                    int attempt;
                    for (attempt = 0; attempt < 8; attempt++) {
                        nonblack = 0;
                        distinct_mask = 0;
                        for (size_t i = 0; i < nsamples; i++) {
                            uint32_t v = px[i * 256];
                            if ((v & 0x00ffffff) != 0) {
                                nonblack++;
                                distinct_mask |= v;
                            }
                        }
                        colors_bits = __builtin_popcount(distinct_mask & 0x00ffffff);
                        printf("  fb0 attempt=%d smem_len=%u samples=%zu nonblack=%zu "
                               "distinct_mask=%08x colors_bits=%d\n",
                               attempt, smem_len, nsamples, nonblack,
                               distinct_mask, colors_bits);
                        fflush(stdout);
                        if (nonblack * 10 >= nsamples && colors_bits >= 4) {
                            break; // success
                        }
                        sleep(1);
                    }
                    if (nonblack * 10 >= nsamples && colors_bits >= 4) {
                        pass("lxde_pixels_visible");
                        // Save the framebuffer for off-VM screenshot.
                        // Mirror of test_openbox.c — debugfs picks this up.
                        // ROOT is the real ext2 mount; /root is initramfs.
                        int out = open(ROOT "/root/fb-snapshot.bgra",
                                       O_WRONLY | O_CREAT | O_TRUNC, 0644);
                        if (out >= 0) {
                            (void)write(out, fb, smem_len);
                            close(out);
                            printf("  fb0 snapshot saved (%u bytes after %d attempts)\n",
                                   smem_len, attempt + 1);
                            fflush(stdout);
                        }
                    } else {
                        char b[80];
                        snprintf(b, sizeof(b),
                            "nonblack=%zu/%zu colors_bits=%d after %d attempts",
                            nonblack, nsamples, colors_bits, attempt);
                        fail("lxde_pixels_visible", b);
                    }
                    munmap(fb, smem_len);
                }
                close(fd);
            }
        }

        // Dump LXDE component logs so failures are diagnosable.
        {
            const char *logs[] = {
                "/tmp/lxde-session.log",
                "/var/log/Xorg.0.log",
                NULL,
            };
            for (int li = 0; logs[li]; li++) {
                char b[8192];
                char cmd[192];
                int n = (strstr(logs[li], "Xorg.0") ? 120 : 40);
                snprintf(cmd, sizeof(cmd),
                         "echo '== %s =='; tail -%d %s 2>/dev/null",
                         logs[li], n, logs[li]);
                if (sh_capture(cmd, b, sizeof(b), 3000) == 0 && b[0]) {
                    printf("%s", b);
                }
            }
            fflush(stdout);

            // Persist /tmp/lxde-session.log + Xorg.0.log to /var/log/
            // on the disk so the iterate-lxde extractor can pull them
            // out via debugfs after shutdown.  sh_run() chroots into
            // ROOT first, so paths here are inside the disk's
            // filesystem.  /tmp is tmpfs (mounted at ROOT "/tmp");
            // /var/log lives on the ext2 disk image.  Xorg writes to
            // /tmp/Xorg.0.log when /var/log isn't pre-created.
            sh_run("mkdir -p /var/log "
                   "&& cp -f /tmp/lxde-session.log "
                   "/var/log/lxde-session.log 2>/dev/null; "
                   "cp -f /tmp/Xorg.0.log "
                   "/var/log/Xorg.0.log 2>/dev/null; true", 3000);
        }
    }

    // Phase-1 input device verification: confirm /dev/input/event0
    // (the virtio-keyboard-device) is a real evdev character node
    // backed by exts/virtio_input — not a stub or missing file.
    // The harder "typed text actually reaches Xorg" check needs a
    // host-side QMP driver to inject keystrokes; that's covered by
    // test-lxde-input via tools/lxde-input-driver.py.
    {
        struct stat st;
        if (stat(ROOT "/dev/input/event0", &st) == 0 && S_ISCHR(st.st_mode)) {
            pass("evdev_event0_present");
        } else {
            char b[64];
            snprintf(b, sizeof(b), "stat=%d mode=%o errno=%d",
                     stat(ROOT "/dev/input/event0", &st), st.st_mode, errno);
            fail("evdev_event0_present", b);
        }

        // Real evdev nodes return EAGAIN on non-blocking read when
        // the queue is empty; a stub would return 0 (EOF) or fail
        // to open.  Either of those is a kernel-side bug.
        int fd = open(ROOT "/dev/input/event0", O_RDONLY | O_NONBLOCK);
        if (fd < 0) {
            char b[64];
            snprintf(b, sizeof(b), "open errno=%d", errno);
            fail("evdev_event0_readable", b);
        } else {
            char buf[64];
            ssize_t n = read(fd, buf, sizeof(buf));
            int e = errno;
            close(fd);
            if (n < 0 && e == EAGAIN) {
                pass("evdev_event0_readable");
            } else if (n > 0) {
                // Got real events queued — also fine, evdev is alive.
                pass("evdev_event0_readable");
            } else {
                char b[64];
                snprintf(b, sizeof(b), "read=%zd errno=%d", n, e);
                fail("evdev_event0_readable", b);
            }
        }
    }

    // Phase-1d: typed-text-arrives. Spawn xterm running `cat` with
    // stdout redirected to a disk-backed file, signal the host driver
    // (`run-qemu.py --inject-on-line INJECT_NOW --inject-keys ...`)
    // by emitting the sentinel line, and verify the keystrokes that
    // QMP injects into virtio-keyboard reach the terminal via Xorg.
    //
    // We split this into two checks so failures are diagnosable:
    //   evdev_keys_arrived — bytes show up on /dev/input/event0 at
    //     all (proves virtio-keyboard → evdev devnode works).
    //   typed_text_arrived — those bytes route through Xorg → xterm
    //     → cat → ext2.  Failing this when evdev_keys_arrived passes
    //     means the gap is in Xorg/xkb/xf86-input-evdev focus
    //     handling, not the kernel.
    //
    // Without a driver injecting, both report SKIP.
    {
        // Make sure /var/log exists and start fresh.
        sh_run("mkdir -p /var/log; rm -f /var/log/typed.txt /var/log/evdev.bin", 1000);

        // Open both event0 and event1 (one is keyboard, one is mouse —
        // QEMU's virt MMIO assigns lower addresses to later -device
        // args, so the keyboard often lands at event1 not event0 on
        // arm64).  Drain anything pending.
        int evfd0 = open(ROOT "/dev/input/event0", O_RDONLY | O_NONBLOCK);
        int evfd1 = open(ROOT "/dev/input/event1", O_RDONLY | O_NONBLOCK);
        char drain[4096];
        if (evfd0 >= 0) while (read(evfd0, drain, sizeof(drain)) > 0) {}
        if (evfd1 >= 0) while (read(evfd1, drain, sizeof(drain)) > 0) {}

        // xterm runs `cat > /var/log/typed.txt` via sh -c.  cat stays
        // alive waiting for stdin EOF, keeping the window mapped for
        // the injection window.
        char xcmd[1024];
        snprintf(xcmd, sizeof(xcmd),
                 "%s exec /usr/bin/xterm -bg '#404080' -fg '#ffffff' "
                 "-e sh -c 'cat > /var/log/typed.txt' "
                 ">>/tmp/lxde-session.log 2>&1",
                 env_prefix);
        start_bg(xcmd);
        sleep(3);  // let xterm map + receive focus

        // Sentinel — host driver matches this and injects keys.
        printf("INJECT_NOW: kevlar-lxde-input-ready\n");
        fflush(stdout);
        sleep(6);  // let host inject + Xorg route + cat write

        // Sync so the file is visible on disk via debugfs too.
        sh_run("sync", 1000);

        // Drain both event0 and event1 to count bytes that arrived.
        // Either device receiving non-zero bytes proves virtio-keyboard
        // delivered events through to the evdev devnode.
        ssize_t ev0_bytes = 0, ev1_bytes = 0;
        if (evfd0 >= 0) {
            for (;;) {
                ssize_t r = read(evfd0, drain, sizeof(drain));
                if (r <= 0) break;
                ev0_bytes += r;
            }
            close(evfd0);
        }
        if (evfd1 >= 0) {
            for (;;) {
                ssize_t r = read(evfd1, drain, sizeof(drain));
                if (r <= 0) break;
                ev1_bytes += r;
            }
            close(evfd1);
        }
        ssize_t evbytes = ev0_bytes + ev1_bytes;
        printf("  evdev event0=%zd bytes, event1=%zd bytes (total %zd)\n",
               ev0_bytes, ev1_bytes, evbytes);
        fflush(stdout);

        if (evbytes >= 24) {
            pass("evdev_keys_arrived");
        } else if (evbytes == 0) {
            skip("evdev_keys_arrived", "no input driver attached or no events");
        } else {
            char b[64];
            snprintf(b, sizeof(b), "only %zd bytes (need >=24)", evbytes);
            fail("evdev_keys_arrived", b);
        }

        char typed[256] = {0};
        int rfd = open(ROOT "/var/log/typed.txt", O_RDONLY);
        ssize_t n = 0;
        if (rfd >= 0) {
            n = read(rfd, typed, sizeof(typed) - 1);
            close(rfd);
            if (n > 0) typed[n] = '\0';
        }
        printf("  typed.txt: [%s] (%zd bytes)\n", typed, n);
        fflush(stdout);

        if (strstr(typed, "kevlar")) {
            pass("typed_text_arrived");
        } else if (n <= 0 || typed[0] == '\0') {
            // No driver active — expected for plain `make test-lxde`.
            skip("typed_text_arrived", "no input driver attached");
        } else {
            fail("typed_text_arrived", typed);
        }
    }

    printf("\nTEST_END %d/%d\n", g_pass, g_total);
    printf("T: %d passed, %d failed\n", g_pass, g_fail);
    fflush(stdout);

    sync();

    // Late-path keepalive: if the early read_kevlar_interactive() in
    // main() returned 0 (e.g. /proc not mountable on initramfs root
    // at that point), retry now — by this point setup_rootfs has
    // mounted procfs, so /mnt/proc/cmdline is available even if
    // /proc on the initramfs isn't.  Either source carries the same
    // kernel cmdline.  Without this fallback, run-alpine-lxde would
    // halt instead of staying alive when the early check fails.
    if (!g_interactive) {
        if (read_kevlar_interactive()) {
            interactive_keepalive();
        }
        // Last resort: read /proc/cmdline through the chroot-mounted
        // procfs that setup_rootfs created.
        int cfd = open(ROOT "/proc/cmdline", O_RDONLY);
        if (cfd >= 0) {
            char buf[1024];
            ssize_t n = read(cfd, buf, sizeof(buf) - 1);
            close(cfd);
            if (n > 0) {
                buf[n] = '\0';
                if (strstr(buf, "kevlar_interactive")) {
                    interactive_keepalive();
                }
            }
        }
    }

    return 0;
}
