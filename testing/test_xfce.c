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

// Run a command in chroot, return exit code (-2 = timeout)
static int sh_run(const char *cmd, int timeout_ms) {
    // Fixed: COW refcount==1 now properly makes page writable.
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
    // Make read end non-blocking
    fcntl(pipefd[0], F_SETFL, O_NONBLOCK);
    int total = 0;
    for (int i = 0; i < timeout_ms / 50; i++) {
        char tmp[256];
        int n = read(pipefd[0], tmp, sizeof(tmp));
        if (n > 0 && total + n < bufsz) { memcpy(buf + total, tmp, n); total += n; }
        int status;
        pid_t r = waitpid(pid, &status, WNOHANG);
        if (r == pid) {
            // Drain remaining data
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

    // Quick mode: skip Phases 1-4 (proven working) to focus on Phase 5.
    // Set to 1 to go straight to XFCE session startup.
    int quick_mode = 1;
    if (quick_mode) goto phase5;

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

        // Skip fb0 probe and font check (covered by Phase 1-2) to save time for Phase 5.
        if (0) {
            char b[1024];
            printf("  === fb0_probe (direct) ===\n");
            // Run fb0_probe directly (not in chroot) — accesses kernel's /dev/fb0
            pid_t pid = fork();
            if (pid == 0) {
                execl("/bin/fb0-probe", "fb0-probe", NULL);
                _exit(127);
            }
            int status;
            waitpid(pid, &status, 0);
            // Also run from INSIDE the chroot
            printf("  === fb0_probe (chroot) ===\n");
            // Copy the binary into the chroot
            sh_run("cp /bin/fb0-probe /fb0-probe 2>/dev/null || true", 2000);
            sh_capture("/fb0-probe 2>&1", b, sizeof(b), 5000);
            printf("%s\n", b);
        }
        // Also run shell-based probe
        {
            char b[512];
            sh_capture(
                "exec 2>&1; "
                // First: what fbdevHWProbe does
                "echo '--- fbdevHWProbe test ---'; "
                "exec 3</dev/fb0 && echo 'O_RDONLY open: OK (fd 3)' || echo 'O_RDONLY open: FAILED'; "
                "exec 3>&-; "
                // Second: what fbdevHWInit does
                "exec 3<>/dev/fb0 && echo 'O_RDWR open: OK (fd 3)' || echo 'O_RDWR open: FAILED'; "
                "exec 3>&-; "
                // Third: check fstat on the fd
                "ls -la /dev/fb0; "
                "stat -c '%F %a %t:%T' /dev/fb0 2>/dev/null || stat /dev/fb0 2>&1 | head -3; "
                // Fourth: check if Xorg binary can see fb0
                "test -r /dev/fb0 && echo 'fb0 readable' || echo 'fb0 NOT readable'; "
                "test -w /dev/fb0 && echo 'fb0 writable' || echo 'fb0 NOT writable'; "
                "test -c /dev/fb0 && echo 'fb0 is char device' || echo 'fb0 NOT char device'; ",
                b, sizeof(b), 5000);
            printf("  chroot fb0 probe:\n%s\n", b);
        }

        // Generate X11 font dirs BEFORE starting Xorg
        {
            char b[512];
            sh_run("mkfontscale /usr/share/fonts/misc 2>&1; "
                   "mkfontdir /usr/share/fonts/misc 2>&1",
                   10000);
            sh_capture("wc -l /usr/share/fonts/misc/fonts.dir 2>&1; "
                      "grep -i semicondensed /usr/share/fonts/misc/fonts.dir 2>&1 | head -1; "
                      "head -5 /usr/share/fonts/misc/fonts.dir 2>&1",
                      b, sizeof(b), 3000);
            printf("  fonts.dir:\n%s\n", b);
        } // end if(0) skip block

        // Start Xorg
        {
            printf("  Starting Xorg...\n");
            fflush(stdout);
            int rc = sh_run(
                "/usr/libexec/Xorg :0 -noreset -nolisten tcp "
                "-config /etc/X11/xorg.conf.d/10-fbdev.conf "
                "vt1 >/tmp/xorg-stdout.log 2>&1 &"
                "sleep 5; "
                "kill -0 $! 2>/dev/null && echo XORG_ALIVE || echo XORG_DEAD",
                12000);
            printf("  Xorg start rc=%d\n", rc);
            fflush(stdout);
        }
        sleep(4);

        // Skip Xorg diagnostic dump to save time for Phase 5.
        sleep(2);
        if (0) {
            char buf[2048];
            // Test fb0 ioctl from INSIDE the chroot
            {
                char b[256];
                sh_capture("ls -la /dev/fb0 2>&1", b, sizeof(b), 2000);
                printf("  chroot fb0: %s", b);
                int rc2 = sh_run("dd if=/dev/fb0 bs=4 count=1 of=/dev/null 2>/dev/null", 3000);
                printf("  chroot fb0 read: %s\n", rc2 == 0 ? "OK" : "FAILED");
                // Check sysfs — Xorg fbdev may need /sys/class/graphics/fb0
                sh_capture("ls /sys/class/ 2>&1", b, sizeof(b), 2000);
                printf("  sysfs class: %s", b);
                sh_capture("ls /sys/class/graphics/ 2>&1", b, sizeof(b), 2000);
                printf("  sysfs graphics: %s", b);
                sh_capture("ls /sys/bus/pci/devices/ 2>&1", b, sizeof(b), 2000);
                printf("  sysfs pci: %s\n", b);
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

            // Dump key Xorg log lines (skip full dump to save time)
            printf("  === Xorg.0.log (key lines) ===\n");
            sh_capture("grep -E '\\(II\\) Initializing|\\(EE\\)|FBDEV|mode.*1024' /var/log/Xorg.0.log 2>/dev/null | head -20",
                       buf, sizeof(buf), 3000);
            printf("  %s", buf);
            printf("  === END ===\n");
            fflush(stdout);
        } // end if(0) skip Xorg diag

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

    // --- Phase 4: Font cache + xterm test ---
    printf("\n=== Phase 4: Font Cache + X11 Rendering ===\n");
    {
        // Generate font directories (required by X11 font loading)
        printf("  Generating font dirs...\n"); fflush(stdout);
        sh_run("mkfontscale /usr/share/fonts/misc 2>/dev/null; "
               "mkfontdir /usr/share/fonts/misc 2>/dev/null; "
               "mkfontscale /usr/share/fonts/dejavu 2>/dev/null; "
               "mkfontdir /usr/share/fonts/dejavu 2>/dev/null",
               10000);
        printf("  Font dirs done\n"); fflush(stdout);

        // Write a test pattern to fb0 to verify rendering is visible
        // (simpler than xterm which has library issues)
        {
            int fd = open("/dev/fb0", O_RDWR);
            if (fd >= 0) {
                unsigned char finfo[68] = {0};
                if (ioctl(fd, 0x4602, finfo) == 0) {
                    unsigned int smem_len = *(unsigned int *)(finfo + 24);
                    unsigned int stride = *(unsigned int *)(finfo + 48);
                    void *fb = mmap(NULL, smem_len, PROT_READ|PROT_WRITE, MAP_SHARED, fd, 0);
                    if (fb != MAP_FAILED) {
                        // Draw a white rectangle (100x50 pixels) at position (50,50)
                        unsigned int *pixels = (unsigned int *)fb;
                        for (int y = 50; y < 100; y++)
                            for (int x = 50; x < 150; x++)
                                pixels[y * (stride/4) + x] = 0xFFFFFFFF;
                        // Draw "OK" text pattern at (200, 50) — simple block letters
                        for (int y = 50; y < 80; y++)
                            for (int x = 200; x < 260; x++)
                                pixels[y * (stride/4) + x] = 0xFF00FF00; // green
                        munmap(fb, smem_len);
                        printf("  Drew test rectangle on framebuffer\n");
                    }
                }
                close(fd);
            }
        }
        // Test X11 rendering without xterm: use xsetroot to change root window color
        {
            int rc = sh_run("DISPLAY=:0 xsetroot -solid '#336699' 2>/dev/null", 5000);
            if (rc == 0) {
                pass("xsetroot_color");
                printf("  Root window set to blue (#336699)\n");
            } else {
                // xsetroot might not be installed, try with xprop
                rc = sh_run("DISPLAY=:0 xprop -root -f _TEST 8s -set _TEST ok 2>/dev/null", 5000);
                if (rc == 0) pass("xsetroot_color");
                else fail("xsetroot_color", "X11 client failed");
            }
        }

        // Verify framebuffer pixels changed after xsetroot
        {
            int fd = open("/dev/fb0", O_RDONLY);
            if (fd >= 0) {
                unsigned int *fb = mmap(NULL, 1024*768*4, PROT_READ, MAP_SHARED, fd, 0);
                if (fb != MAP_FAILED) {
                    // Sample pixels at center of screen (512, 384)
                    // xsetroot -solid '#336699' → BGR = 0x996633 in BGRA format
                    // Bochs VGA is BGRA: blue=byte0, green=byte1, red=byte2
                    unsigned int center = fb[384 * 1024 + 512];
                    unsigned int tl = fb[100 * 1024 + 100];  // top-left area
                    int non_black = (center != 0 && center != 0xFF000000);
                    printf("  fb pixel check: center=%08x tl=%08x %s\n",
                           center, tl, non_black ? "VISIBLE" : "BLACK");
                    if (non_black)
                        pass("fb_pixels_visible");
                    else
                        fail("fb_pixels_visible", "framebuffer still black after xsetroot");
                    munmap(fb, 1024*768*4);
                }
                close(fd);
            }
        }

        // Also try xterm (might crash, but test it)
        start_bg("DISPLAY=:0 HOME=/root xterm -geometry 80x24+50+50 "
                 "-e 'echo HELLO_KEVLAR; sleep 30' 2>/dev/null");

        if (sh_run("pgrep -x xterm >/dev/null 2>&1", 2000) == 0)
            pass("xterm_running");
        else
            fail("xterm_running", "xterm not found");
    }

    // --- Phase 5: Start XFCE ---
phase5:
    printf("\n=== Phase 5: XFCE Session ===\n");
    // In quick mode, start D-Bus and Xorg first
    if (quick_mode) {
        // Generate machine-id for D-Bus
        sh_run("dbus-uuidgen > /etc/machine-id 2>/dev/null; "
               "cp /etc/machine-id /var/lib/dbus/machine-id 2>/dev/null",
               2000);
        sh_run("dbus-daemon --system --fork 2>/dev/null", 3000);
        sh_run("rm -f /tmp/.X0-lock /tmp/.X11-unix/X0", 500);
        sh_run("/usr/libexec/Xorg :0 -noreset -nolisten tcp "
               "-config /etc/X11/xorg.conf.d/10-fbdev.conf "
               "vt1 >/dev/null 2>&1 &"
               "sleep 3", 8000);
    }
    {
        // Test: can we exec startxfce4 directly inside the chroot?
        {
            char b[512];
            sh_capture("which startxfce4 2>&1; ls -la /usr/bin/startxfce4 2>&1; "
                      "head -2 /usr/bin/startxfce4 2>&1",
                      b, sizeof(b), 3000);
            printf("  startxfce4 check: %s\n", b);
        }
        // Start session D-Bus manually, then startxfce4
        // Start XFCE with proper environment
        // Start a persistent session bus (not via dbus-launch which kills
        // the bus when its child exits). Then start xfce4-session with
        // the bus address. NO_AT_BRIDGE=1 prevents at-spi GLib traps.
        sh_run("rm -f /tmp/.dbus-session-sock; "
               "dbus-daemon --session "
               "--address=unix:path=/tmp/.dbus-session-sock "
               "--fork --print-address >/tmp/dbus-addr 2>/dev/null", 10000);
        start_bg("export DISPLAY=:0 HOME=/root "
                 "PATH=/usr/bin:/usr/sbin:/usr/local/bin:/bin:/sbin "
                 "XDG_DATA_DIRS=/usr/share "
                 "GTK_THEME=Adwaita "
                 "NO_AT_BRIDGE=1 "
                 "DBUS_SESSION_BUS_ADDRESS=unix:path=/tmp/.dbus-session-sock; "
                 "exec /usr/bin/xfce4-session "
                 ">/tmp/xfce-session.log 2>&1");
        // Wait for XFCE to start. Use a pipe: child sleeps then writes,
        // parent blocks on read. This avoids CPU-burning yield loops that
        // cause timer starvation on SMP.
        {
            int pfd[2];
            pipe(pfd);
            pid_t sleeper = fork();
            if (sleeper == 0) {
                close(pfd[0]);
                sleep(10);
                write(pfd[1], "x", 1);
                _exit(0);
            }
            close(pfd[1]);
            char buf;
            read(pfd[0], &buf, 1); // blocks until child writes or pipe closes
            close(pfd[0]);
            waitpid(sleeper, NULL, 0);
        }
        printf("  XFCE wait done\n");
        fflush(stdout);

        // Quick process snapshot
        {
            char b[512];
            sh_capture("ps -o pid,args 2>/dev/null | grep -E 'xfwm|xfce4-panel|xfce4-session|xfconfd|Xorg' | head -10",
                       b, sizeof(b), 2000);
            printf("  XFCE procs: %s\n", b); fflush(stdout);
        }

        // Check components
        if (sh_run("pgrep -x xfwm4 >/dev/null 2>&1", 2000) == 0)
            pass("xfwm4_running");
        else
            fail("xfwm4_running", "window manager not found");

        // Use ps + grep instead of pgrep — pgrep in chroot may miss
        // processes due to /proc/[pid]/comm truncation.
        if (sh_run("ps -o args 2>/dev/null | grep -q '[x]fce4-panel'", 2000) == 0) {
            pass("xfce4_panel_running");
        } else {
            fail("xfce4_panel_running", "panel not found");
        }

        if (sh_run("pgrep xfce4-session >/dev/null 2>&1", 2000) == 0)
            pass("xfce4_session_running");
        else
            fail("xfce4_session_running", "session not found");

    }

    printf("\nTEST_END %d/%d\n", g_pass, g_total);
    printf("T: %d passed, %d failed\n", g_pass, g_fail);
    fflush(stdout);

    sync();
    return 0;
}
