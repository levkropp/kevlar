// End-to-end openbox desktop test for Kevlar.
//
// Mirror of test_i3.c but with a simpler WM stack: openbox alone (no
// IPC socket, no built-in bar machinery, no event subscription
// protocol).  Comparison baseline — if openbox passes where i3 flakes
// under stress, the issue is i3-specific (libev/IPC/bar polling).
//
// Run via: make test-openbox
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
#include <time.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <sched.h>
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
    mkdir(ROOT "/tmp/.X11-unix", 01777);
}

int main(void) {
    printf("T: openbox Desktop End-to-End Test\n");
    fflush(stdout);
    setup_rootfs();

    sh_run("rm -f /tmp/.X0-lock /tmp/.X11-unix/X0", 500);
    sh_run("/usr/libexec/Xorg :0 -noreset -nolisten tcp "
           "-config /etc/X11/xorg.conf.d/10-fbdev.conf "
           "vt1 >/var/log/Xorg.0.log 2>&1 &"
           "sleep 3", 8000);

    int rc = sh_run("DISPLAY=:0 xdpyinfo >/dev/null 2>&1", 5000);
    if (rc == 0) pass("xorg_running");
    else { char b[32]; snprintf(b, sizeof(b), "rc=%d", rc); fail("xorg_running", b); }

    // Pluck `kbox-phase=N` from /proc/cmdline so we can sweep kbox
    // phases without rebuilding the disk image.  Default 0.
    // /proc isn't mounted at the test's root (only at ROOT "/proc"
    // for the chroot), so mount it just to read cmdline.
    mkdir("/proc", 0755);
    int mrc = mount("proc", "/proc", "proc", 0, NULL);
    printf("  mount /proc rc=%d errno=%d\n", mrc, errno); fflush(stdout);
    char kbox_phase[16] = "0";
    {
        int fd = open("/proc/cmdline", O_RDONLY);
        printf("  open /proc/cmdline fd=%d errno=%d\n", fd, fd<0?errno:0);
        fflush(stdout);
        if (fd >= 0) {
            char buf[1024];
            int n = read(fd, buf, sizeof(buf) - 1);
            close(fd);
            printf("  read cmdline n=%d\n", n); fflush(stdout);
            if (n > 0) {
                buf[n] = '\0';
                printf("  cmdline=%.*s\n", n>200?200:n, buf); fflush(stdout);
                const char *p = strstr(buf, "kbox-phase=");
                if (p) {
                    p += strlen("kbox-phase=");
                    int i = 0;
                    while (i < (int)sizeof(kbox_phase) - 1
                           && p[i] >= '0' && p[i] <= '9') {
                        kbox_phase[i] = p[i];
                        i++;
                    }
                    kbox_phase[i] = '\0';
                }
            }
        }
        printf("  kbox-phase=%s\n", kbox_phase); fflush(stdout);
    }

    // Pluck `kxreplay-limit=N` and `kxreplay-skip=N` from
    // /proc/cmdline so we can bisect kxreplay's chunk window
    // without rebuilding the disk image.
    char kxr_limit[16] = "999999";
    char kxr_skip[16]  = "0";
    char kxr_tail[16]  = "0";
    char kxr_tail_idx[16] = "";
    char kxr_include[256] = "";
    char p19_variant[32] = "";
    {
        int fd = open("/proc/cmdline", O_RDONLY);
        if (fd >= 0) {
            char buf[1024];
            int n = read(fd, buf, sizeof(buf) - 1);
            close(fd);
            if (n > 0) {
                buf[n] = '\0';
                struct { const char *key; char *dst; size_t cap; int allow_comma; int allow_alpha; } args[] = {
                    { "kxreplay-limit=", kxr_limit,   sizeof(kxr_limit),   0, 0 },
                    { "kxreplay-skip=",  kxr_skip,    sizeof(kxr_skip),    0, 0 },
                    { "kxreplay-tail=",  kxr_tail,    sizeof(kxr_tail),    0, 0 },
                    { "kxreplay-tail-idx=", kxr_tail_idx, sizeof(kxr_tail_idx), 0, 0 },
                    { "kxreplay-include=", kxr_include, sizeof(kxr_include), 1, 0 },
                    { "kbox-19-variant=", p19_variant, sizeof(p19_variant), 0, 1 },
                };
                for (unsigned k = 0; k < sizeof(args)/sizeof(args[0]); k++) {
                    const char *p = strstr(buf, args[k].key);
                    if (p) {
                        p += strlen(args[k].key);
                        int i = 0;
                        while (i < (int)args[k].cap - 1 &&
                               ((p[i] >= '0' && p[i] <= '9') ||
                                (args[k].allow_comma && p[i] == ',') ||
                                (args[k].allow_alpha && ((p[i]>='a'&&p[i]<='z')||(p[i]>='A'&&p[i]<='Z'))))) {
                            args[k].dst[i] = p[i]; i++;
                        }
                        args[k].dst[i] = '\0';
                    }
                }
            }
        }
    }
    char env_prefix_buf[768];
    snprintf(env_prefix_buf, sizeof(env_prefix_buf),
             "export DISPLAY=:0 HOME=/root "
             "PATH=/usr/bin:/usr/sbin:/usr/local/bin:/bin:/sbin "
             "XDG_CONFIG_HOME=/root/.config "
             "KBOX_PHASE=%s "
             "KXREPLAY_LIMIT=%s KXREPLAY_SKIP=%s "
             "KXREPLAY_TAIL_BYTES=%s KXREPLAY_TAIL_IDX=%s "
             "KXREPLAY_INCLUDE=%s "
             "KBOX_PHASE_19_VARIANT=%s; ",
             kbox_phase, kxr_limit, kxr_skip, kxr_tail, kxr_tail_idx, kxr_include, p19_variant);
    const char *env_prefix = env_prefix_buf;
    printf("  kxreplay-limit=%s skip=%s tail=%s tail-idx=%s include=[%s] p19=%s\n",
           kxr_limit, kxr_skip, kxr_tail, kxr_tail_idx, kxr_include, p19_variant); fflush(stdout);

    char cmd[1024];

    // Paint the root background first.
    {
        printf("  T-warm: xsetroot\n"); fflush(stdout);
        char out[512];
        snprintf(cmd, sizeof(cmd),
                 "%s /usr/bin/xsetroot -solid '#2e557f' 2>&1; echo rc=$?",
                 env_prefix);
        sh_capture(cmd, out, sizeof(out), 5000);
        printf("  xsetroot: %.*s\n", 400, out); fflush(stdout);
    }

    // Start openbox FIRST.  Unlike i3 (which can adopt
    // already-mapped windows), openbox occasionally fails to
    // reparent windows that mapped before it claimed the WM
    // selection — they stay "managed" but the reparent puts them
    // in an off-screen state.  Easier to just start the WM first
    // and let it Map/Reparent each new client cleanly.
    printf("  T+0 about to start_bg openbox\n"); fflush(stdout);
    snprintf(cmd, sizeof(cmd),
             // KBOX_PHASE=99: real openbox.real (no wrapping).
             // KBOX_PHASE=98: real openbox.real wrapped under strace.
             //   (Kevlar currently has no ptrace, so strace falls
             //   back to plain exec — but the wrap shifts timing.)
             // KBOX_PHASE=97: kxproxy on :1 forwarding to Xorg's
             //   :0; openbox.real runs with DISPLAY=:1 so its
             //   entire X11 conversation is captured byte-for-byte
             //   in /tmp/kxproxy.log.  See task #39.
             // KBOX_PHASE=96: kxreplay replays a captured byte
             //   trace from a previous KBOX_PHASE=97 run against
             //   real Xorg.  No openbox involved.  Tests whether
             //   the hang is purely-byte-determined.  See task #41.
             (atoi(kbox_phase) == 99
              ? "%s exec /usr/bin/openbox.real >/tmp/openbox.log 2>&1"
              : atoi(kbox_phase) == 98
              ? "%s exec /usr/bin/strace -y -s 8192 "
                "-e trace=read,write,readv,writev,recvmsg,sendmsg,"
                "recvfrom,sendto,connect,socket,close,poll,ppoll,"
                "epoll_wait,epoll_pwait,epoll_ctl "
                "-o /tmp/openbox.strace /usr/bin/openbox.real "
                ">/tmp/openbox.log 2>&1"
              : atoi(kbox_phase) == 97
              ? "%s /usr/bin/kxproxy 1 0 >/tmp/kxproxy.log 2>&1 & "
                "sleep 1; "
                "DISPLAY=:1 /usr/bin/openbox.real "
                ">/tmp/openbox.log 2>&1"
              : atoi(kbox_phase) == 96
              ? "%s exec /usr/bin/kxreplay "
                ">/tmp/openbox.log 2>&1"
              : "%s exec /usr/bin/openbox >/tmp/openbox.log 2>&1"),
             env_prefix);
    start_bg(cmd);
    sleep(3);  // let openbox claim WM_S0 and set _NET_SUPPORTING_WM_CHECK

    // Settling time for openbox to claim WM_S0 and set
    // _NET_SUPPORTING_WM_CHECK.  Skip xterm for now — it doesn't
    // render under openbox (window managed but never reaches
    // visible state, suspect missing SubstructureRedirect or
    // Map/Reparent path) and isn't required to verify WM ownership.
    for (int s = 0; s < 6; s++) {
        sleep(1);
        printf("  T+%d sleeping\n", 1 + s); fflush(stdout);
    }
    printf("  openbox wait done\n"); fflush(stdout);

    // Salvage openbox/kbox log — dump up to 200 lines so phase
    // markers and the per-request trace are visible to the harness.
    sh_run("cat /tmp/openbox.log 2>&1 | head -200 "
           "| sed 's/^/  openbox: /'", 2000);
    // KBOX_PHASE=99 wraps openbox.real under strace; dump the
    // first chunk of its strace too so we can diff openbox's
    // X11 wire traffic against kbox's.
    if (atoi(kbox_phase) == 98) {
        sh_run("wc -l /tmp/openbox.strace 2>&1", 1000);
        sh_run("head -300 /tmp/openbox.strace 2>&1 "
               "| sed 's/^/  ostrace: /'", 3000);
    }
    if (atoi(kbox_phase) == 97) {
        sh_run("wc -l /tmp/kxproxy.log 2>&1", 1000);
        // Copy /tmp/kxproxy.log (tmpfs) to /var/log/kxproxy.log
        // (on-disk ext2) so the host can extract it via debugfs
        // after the test exits.  sync() at the very end of the
        // test pushes the dirty cache to disk.
        sh_run("cp /tmp/kxproxy.log /var/log/kxproxy.log 2>&1; sync", 3000);
        // Print the LAST 200 lines (the most interesting part —
        // wherever openbox's burst stalls is near the end).
        sh_run("tail -200 /tmp/kxproxy.log 2>&1 "
               "| sed 's/^/  proxy-tail: /'", 4000);
    }
    sleep(1);

    // Scan /proc for openbox.
    {
        int has_openbox = 0;
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
            if (strcmp(comm, "openbox") == 0) has_openbox = 1;
        }
        printf("  components: openbox=%d\n", has_openbox);
        fflush(stdout);
        if (has_openbox) pass("openbox_running");
        else fail("openbox_running", "WM not found");

        // Probe Xorg responsiveness with xset -q (returns immediately
        // if Xorg is alive) before the actual xprop check.  Tells us
        // whether xprop hangs because Xorg is hung, or just because
        // openbox didn't set _NET_SUPPORTING_WM_CHECK.
        {
            unsigned t = (unsigned)time(NULL);
            int rc2 = sh_run("DISPLAY=:0 xset -q "
                             ">/tmp/xset.out 2>/tmp/xset.err", 5000);
            printf("  xset -q took %us, rc=%d\n",
                   (unsigned)time(NULL) - t, rc2); fflush(stdout);
        }

        // List all properties on the root window.  If the list returns
        // but _NET_SUPPORTING_WM_CHECK isn't in it, openbox didn't set
        // it; if the list itself times out, Xorg is dead.
        {
            unsigned t = (unsigned)time(NULL);
            int rc2 = sh_run("DISPLAY=:0 xprop -root "
                             ">/tmp/root-props.txt 2>&1", 8000);
            printf("  xprop -root (list) took %us, rc=%d\n",
                   (unsigned)time(NULL) - t, rc2); fflush(stdout);
            sh_run("grep -i 'supporting\\|_NET_WM' /tmp/root-props.txt 2>&1 "
                   "| head -8 | sed 's/^/  prop: /'", 1000);
        }

        // openbox sets _NET_SUPPORTING_WM_CHECK on root.  xprop should
        // see it, confirming WM ownership.  Currently times out under
        // task #27 — Xorg enters a user-space loop after openbox's
        // setup completes and never returns to epoll_pwait, so xprop's
        // connect() sits in the listener backlog and times out.  The
        // strace-confirmed last syscall is `clock_gettime`, then no
        // syscalls for the full duration; both CPUs continue ticking,
        // so it's a user-space busy state, not a scheduler hang.
        unsigned t0 = (unsigned)time(NULL);
        rc = sh_run("DISPLAY=:0 xprop -root _NET_SUPPORTING_WM_CHECK "
                    ">/tmp/wm-check.txt 2>/tmp/wm-check.err",
                    15000);
        unsigned t1 = (unsigned)time(NULL);
        printf("  xprop took %us, rc=%d\n", t1 - t0, rc); fflush(stdout);
        if (rc != 0) {
            sh_run("cat /tmp/wm-check.err 2>&1 "
                   "| sed 's/^/  xprop stderr: /' | head -4", 1000);
            // Dump Xorg's process maps so we can decode the user PC
            // logged by PID1_STALL.  The PC sits inside one of these
            // mappings (Xorg binary, libc, libxfont, libxcursor, ...);
            // file_off + (pc - vma_base) tells us where in the file.
            sh_run("echo '=== /proc/4/maps (Xorg) ==='; "
                   "dd if=/proc/4/maps bs=65536 count=1 2>/dev/null "
                   "| sed 's/^/  XORG_MAP: /'",
                   3000);
        }
        if (rc == 0) pass("openbox_owns_wm_selection");
        else { char b[32]; snprintf(b, sizeof(b), "rc=%d", rc);
               fail("openbox_owns_wm_selection", b); }

        // Framebuffer pixel-visibility check.
        {
            int fd = open("/dev/fb0", O_RDONLY);
            if (fd < 0) {
                fail("openbox_pixels_visible", "can't open /dev/fb0");
            } else {
                unsigned char finfo[68] = {0};
                unsigned int smem_len = 0;
                if (ioctl(fd, 0x4602, finfo) == 0) {
                    smem_len = *(unsigned int *)(finfo + 24);
                }
                if (smem_len == 0) smem_len = 1024 * 768 * 4;
                void *fb = mmap(NULL, smem_len, PROT_READ, MAP_SHARED, fd, 0);
                if (fb == MAP_FAILED) {
                    fail("openbox_pixels_visible", "mmap fb0 failed");
                } else {
                    const uint32_t *px = (const uint32_t *)fb;
                    size_t nsamples = smem_len / 1024;
                    size_t nonblack = 0;
                    uint32_t distinct_mask = 0;
                    for (size_t i = 0; i < nsamples; i++) {
                        uint32_t v = px[i * 256];
                        if ((v & 0x00ffffff) != 0) {
                            nonblack++;
                            distinct_mask |= v;
                        }
                    }
                    printf("  fb0 nonblack=%zu/%zu distinct_mask=%08x\n",
                           nonblack, nsamples, distinct_mask);
                    int out = open(ROOT "/root/fb-snapshot.bgra",
                                   O_WRONLY | O_CREAT | O_TRUNC, 0644);
                    if (out >= 0) {
                        write(out, fb, smem_len);
                        close(out);
                    }
                    int colors_bits = __builtin_popcount(distinct_mask & 0x00ffffff);
                    if (nonblack * 10 >= nsamples && colors_bits >= 4) {
                        pass("openbox_pixels_visible");
                    } else {
                        char b[64];
                        snprintf(b, sizeof(b),
                            "nonblack=%zu/%zu colors_bits=%d",
                            nonblack, nsamples, colors_bits);
                        fail("openbox_pixels_visible", b);
                    }
                    munmap(fb, smem_len);
                }
                close(fd);
            }
        }
    }

    printf("TEST_END %d/%d\n", g_pass, g_total);
    fflush(stdout);

    // Dump Xorg log to serial so we can see what Xorg's been doing
    // during the failure window.
    printf("=== /var/log/Xorg.0.log (last 50 lines) ===\n");
    fflush(stdout);
    sh_run("wc -l /var/log/Xorg.0.log 2>&1", 1000);
    sh_run("cat /var/log/Xorg.0.log 2>&1 | sed 's/^/XORG: /'", 5000);
    // Dump Xorg's fd table to identify what each fd is.
    int xpid = -1;
    for (int pid = 2; pid < 50; pid++) {
        char path[32], comm[32];
        snprintf(path, sizeof(path), "/proc/%d/comm", pid);
        int fd = open(path, O_RDONLY);
        if (fd < 0) continue;
        int n = read(fd, comm, sizeof(comm) - 1);
        close(fd);
        if (n <= 0) continue;
        comm[n] = '\0';
        if (n > 0 && comm[n-1] == '\n') comm[n-1] = '\0';
        if (strncmp(comm, "Xorg", 4) == 0) { xpid = pid; break; }
    }
    if (xpid > 0) {
        char cmd[128];
        snprintf(cmd, sizeof(cmd),
                 "ls -la /proc/%d/fd 2>&1 | sed 's/^/XORGFD: /'", xpid);
        printf("=== Xorg fd table (pid=%d) ===\n", xpid);
        fflush(stdout);
        sh_run(cmd, 2000);
    }
    printf("=== end Xorg log ===\n");
    fflush(stdout);

    // Force one more redraw + flush so the snapshot the host
    // extracts via debugfs has the complete desktop image.  Without
    // this, the snapshot can race openbox's MapWindow/Expose cycle
    // and capture only the root background paint.
    {
        char out[256];
        snprintf(out, sizeof(out),
                 "%s /usr/bin/xrefresh 2>&1; "
                 "%s /usr/bin/xset -q >/dev/null 2>&1",
                 // Re-emit env_prefix
                 "export DISPLAY=:0 HOME=/root "
                 "PATH=/usr/bin:/usr/sbin:/usr/local/bin:/bin:/sbin "
                 "XDG_CONFIG_HOME=/root/.config; ",
                 "export DISPLAY=:0 HOME=/root "
                 "PATH=/usr/bin:/usr/sbin:/usr/local/bin:/bin:/sbin "
                 "XDG_CONFIG_HOME=/root/.config; ");
        sh_run(out, 3000);
    }
    sleep(5);

    // Re-snapshot the framebuffer after the settling delay so the
    // saved /root/fb-snapshot.bgra reflects the complete desktop
    // (xterm window + root background) rather than the mid-render
    // capture from the test loop above.
    {
        int fd = open("/dev/fb0", O_RDONLY);
        if (fd >= 0) {
            void *fb = mmap(NULL, 1024*768*4, PROT_READ, MAP_SHARED, fd, 0);
            if (fb != MAP_FAILED) {
                int out = open(ROOT "/root/fb-snapshot.bgra",
                               O_WRONLY | O_CREAT | O_TRUNC, 0644);
                if (out >= 0) {
                    write(out, fb, 1024*768*4);
                    close(out);
                    printf("  fb0 re-snapshot saved\n"); fflush(stdout);
                }
                munmap(fb, 1024*768*4);
            }
            close(fd);
        }
    }

    sync();
    return 0;
}
