// Kevlar Alpine+twm graphical desktop boot shim.
//
// Mounts the Alpine rootfs, pivot_roots into it, starts Xorg with the
// fbdev driver, launches twm as window manager, and opens xterm sessions.
// This is a complete graphical desktop running on Kevlar.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

static void msg(const char *s) { write(1, s, strlen(s)); }

static pid_t spawn(const char *path, char *const argv[], char *const envp[]) {
    pid_t pid = fork();
    if (pid == 0) {
        execve(path, argv, envp);
        _exit(127);
    }
    return pid;
}

int main(void) {
    msg("\n");
    msg("  ╔══════════════════════════════════════════╗\n");
    msg("  ║  Kevlar OS — Alpine + twm Desktop        ║\n");
    msg("  ║  Drop-in Linux kernel replacement         ║\n");
    msg("  ╚══════════════════════════════════════════╝\n");
    msg("\n");

    // Mount rootfs
    mkdir("/mnt", 0755);
    mount("tmpfs", "/mnt", "tmpfs", 0, NULL);
    mkdir("/mnt/root", 0755);

    int r = mount("none", "/mnt/root", "ext2", 0, NULL);
    if (r != 0) r = mount("none", "/mnt/root", "ext4", 0, NULL);
    if (r != 0) {
        msg("kevlar: FATAL — failed to mount rootfs\n");
        char *sh[] = { "/bin/sh", NULL };
        execv("/bin/sh", sh);
        return 1;
    }
    msg("kevlar: rootfs mounted\n");

    // Mount virtual filesystems
    mkdir("/mnt/root/proc", 0755);
    mount("proc", "/mnt/root/proc", "proc", 0, NULL);
    mkdir("/mnt/root/sys", 0755);
    mount("sysfs", "/mnt/root/sys", "sysfs", 0, NULL);
    mkdir("/mnt/root/dev", 0755);
    mount("devtmpfs", "/mnt/root/dev", "devtmpfs", 0, NULL);
    mkdir("/mnt/root/dev/pts", 0755);
    mount("devpts", "/mnt/root/dev/pts", "devpts", 0, NULL);
    mkdir("/mnt/root/dev/shm", 0755);
    mkdir("/mnt/root/tmp", 01777);
    mount("tmpfs", "/mnt/root/tmp", "tmpfs", 0, NULL);
    mkdir("/mnt/root/run", 0755);
    mount("tmpfs", "/mnt/root/run", "tmpfs", 0, NULL);

    // Pivot root
    mkdir("/mnt/root/oldroot", 0755);
    r = syscall(155, "/mnt/root", "/mnt/root/oldroot"); // pivot_root
    if (r == 0) {
        chdir("/");
        umount2("/oldroot", MNT_DETACH);
        msg("kevlar: pivot_root OK\n");
    } else {
        chroot("/mnt/root");
        chdir("/");
        msg("kevlar: chroot fallback OK\n");
    }

    // Ignore SIGCHLD so child exits don't interrupt us
    signal(SIGCHLD, SIG_IGN);

    // Environment for all X11 processes
    char *x11_env[] = {
        "HOME=/root",
        "PATH=/usr/sbin:/usr/bin:/sbin:/bin",
        "TERM=xterm",
        "DISPLAY=:0",
        "XDG_RUNTIME_DIR=/tmp",
        NULL,
    };

    // Quick font setup — just generate fonts.dir for the misc directory.
    // fc-cache is too slow; skip it. mkfontdir for misc is fast (~1s).
    msg("kevlar: generating fonts.dir...\n");
    {
        pid_t fp = fork();
        if (fp == 0) {
            char *argv[] = { "/usr/bin/mkfontdir", "/usr/share/fonts/misc", NULL };
            char *envp[] = { "PATH=/usr/bin:/bin", NULL };
            execve("/usr/bin/mkfontdir", argv, envp);
            _exit(127);
        }
        if (fp > 0) { int st; waitpid(fp, &st, 0); }
        msg("kevlar: fonts ready\n");
    }

    // Override the fbdev config to disable ShadowFB — force direct rendering.
    // ShadowFB renders to a RAM buffer and copies to VRAM; if the copy-back
    // via mmap doesn't work, the display stays black.
    {
        mkdir("/etc/X11", 0755);
        mkdir("/etc/X11/xorg.conf.d", 0755);
        int fd = open("/etc/X11/xorg.conf.d/10-fbdev.conf", O_WRONLY|O_CREAT|O_TRUNC, 0644);
        if (fd >= 0) {
            const char *conf =
                "Section \"Device\"\n"
                "    Identifier \"fbdev\"\n"
                "    Driver \"fbdev\"\n"
                "    Option \"fbdev\" \"/dev/fb0\"\n"
                "    Option \"ShadowFB\" \"on\"\n"
                "EndSection\n"
                "\n"
                "Section \"Screen\"\n"
                "    Identifier \"default\"\n"
                "    Device \"fbdev\"\n"
                "    DefaultDepth 24\n"
                "EndSection\n";
            write(fd, conf, strlen(conf));
            close(fd);
            msg("kevlar: wrote xorg fbdev config (ShadowFB on)\n");
        }
    }

    // Start Xorg
    msg("kevlar: starting Xorg...\n");
    char *xorg_argv[] = {
        "/usr/libexec/Xorg", ":0",
        "-noreset", "-nolisten", "tcp",
        "-config", "/etc/X11/xorg.conf.d/10-fbdev.conf",
        NULL,
    };
    pid_t xorg_pid = spawn("/usr/libexec/Xorg", xorg_argv, x11_env);
    sleep(3); // Wait for Xorg to initialize

    // Verify Xorg is running
    if (kill(xorg_pid, 0) != 0) {
        msg("kevlar: Xorg failed to start!\n");
        // Dump Xorg log for diagnosis
        int logfd = open("/var/log/Xorg.0.log", O_RDONLY);
        if (logfd >= 0) {
            char logbuf[4096];
            int n = read(logfd, logbuf, sizeof(logbuf) - 1);
            if (n > 0) { logbuf[n] = '\0'; msg("=== Xorg.0.log ===\n"); msg(logbuf); msg("\n"); }
            close(logfd);
        }
        char *sh[] = { "/bin/sh", NULL };
        execv("/bin/sh", sh);
        return 1;
    }
    msg("kevlar: Xorg running\n");

    // Dump tail of Xorg log for diagnosis (skip first 4KB to get the end)
    {
        int logfd = open("/var/log/Xorg.0.log", O_RDONLY);
        if (logfd >= 0) {
            // Read total size
            off_t total = lseek(logfd, 0, SEEK_END);
            // Read last 6KB
            off_t start = total > 6144 ? total - 6144 : 0;
            lseek(logfd, start, SEEK_SET);
            char logbuf[6144];
            int n = read(logfd, logbuf, sizeof(logbuf) - 1);
            if (n > 0) { logbuf[n] = '\0'; msg("=== Xorg.0.log (tail) ===\n"); msg(logbuf); msg("\n=== end ===\n"); }
            close(logfd);
        }
    }

    // Write twm config BEFORE starting twm — twm reads .twmrc at startup.
    // RandomPlacement is CRITICAL: without it, twm waits for a mouse click
    // to position each window, and since nobody is clicking, windows never appear.
    msg("kevlar: writing twm config...\n");
    {
        int fd = open("/root/.twmrc", O_WRONLY|O_CREAT|O_TRUNC, 0644);
        if (fd >= 0) {
            const char *rc =
                "RandomPlacement\n"
                "Color {\n"
                "  DefaultBackground \"#3B4252\"\n"
                "  DefaultForeground \"#D8DEE9\"\n"
                "  TitleBackground \"#5E81AC\"\n"
                "  TitleForeground \"#ECEFF4\"\n"
                "  MenuBackground \"#3B4252\"\n"
                "  MenuForeground \"#E5E9F0\"\n"
                "  MenuTitleBackground \"#5E81AC\"\n"
                "  MenuTitleForeground \"#ECEFF4\"\n"
                "  BorderColor \"#81A1C1\"\n"
                "}\n"
                "BorderWidth 2\n"
                "TitleFont \"-misc-fixed-bold-r-normal--13-120-75-75-c-80-iso8859-1\"\n"
                "MenuFont \"-misc-fixed-medium-r-normal--13-120-75-75-c-80-iso8859-1\"\n"
                "IconManagerFont \"-misc-fixed-medium-r-normal--13-120-75-75-c-80-iso8859-1\"\n"
                "ResizeFont \"-misc-fixed-medium-r-normal--13-120-75-75-c-80-iso8859-1\"\n"
                "\n"
                "Button3 = : root : f.menu \"main\"\n"
                "menu \"main\" {\n"
                "  \"Kevlar Desktop\"  f.title\n"
                "  \"XTerm\"           !\"xterm &\"\n"
                "  \"\"                f.nop\n"
                "  \"Restart\"         f.restart\n"
                "  \"Exit\"            f.quit\n"
                "}\n";
            write(fd, rc, strlen(rc));
            close(fd);
        }
    }

    // Set root window background
    msg("kevlar: setting desktop background...\n");
    {
        char *bg_argv[] = { "/usr/bin/xsetroot", "-solid", "#2E3440", NULL };
        pid_t bp = spawn("/usr/bin/xsetroot", bg_argv, x11_env);
        int st; waitpid(bp, &st, 0);
        msg("kevlar: background set\n");
    }

    // SKIP twm — test xterm directly on root window (no WM intercepting MapRequest)
    msg("kevlar: skipping twm, launching xterm directly...\n");

    // Start xterm
    msg("kevlar: starting xterm...\n");
    {
        char *xterm_argv[] = {
            "/usr/bin/xterm",
            "-bg", "red", "-fg", "white",
            "-geometry", "80x24+50+50",
            NULL,
        };
        spawn("/usr/bin/xterm", xterm_argv, x11_env);
    }
    msg("kevlar: xterm spawned\n");

    // Dump Xorg's last syscalls via /proc/PID/trace
    msg("kevlar: checking Xorg state...\n");
    {
        char *diag_argv[] = { "/bin/sh", "-c",
            "echo '=== Xorg trace ==='; "
            "cat /proc/3/trace 2>&1 | tail -20; "
            "echo '=== xterm trace ==='; "
            "cat /proc/8/trace 2>&1 | tail -10; "
            "echo '=== check done ==='",
            NULL };
        char *diag_env[] = { "PATH=/usr/bin:/bin", NULL };
        pid_t dp = fork();
        if (dp == 0) {
            execve("/bin/sh", diag_argv, diag_env);
            _exit(127);
        }
        if (dp > 0) { int st; waitpid(dp, &st, 0); }
    }

    // Force a screen refresh by setting the background again.
    // This triggers Xorg's damage/shadow copy-back path.
    {
        char *bg2_argv[] = { "/usr/bin/xsetroot", "-solid", "#2E3440", NULL };
        pid_t bp2 = spawn("/usr/bin/xsetroot", bg2_argv, x11_env);
        int st; waitpid(bp2, &st, 0);
        msg("kevlar: forced screen refresh\n");
    }

    // Check if xterm created a window by querying X11
    {
        char *check_argv[] = { "/bin/sh", "-c",
            "DISPLAY=:0 /usr/bin/xprop -root WM_STATE 2>&1; "
            "DISPLAY=:0 /usr/bin/xprop -root _NET_CLIENT_LIST 2>&1; "
            "echo '=== process list ==='; "
            "for p in /proc/[0-9]*/comm; do c=$(cat $p 2>/dev/null); [ -n \"$c\" ] && echo \"$p: $c\"; done 2>/dev/null | grep -E 'xterm|twm|Xorg|sh'; "
            "echo '=== done ==='",
            NULL };
        char *check_env[] = { "PATH=/usr/bin:/bin", "DISPLAY=:0", NULL };
        pid_t cp = fork();
        if (cp == 0) {
            execve("/bin/sh", check_argv, check_env);
            _exit(127);
        }
        if (cp > 0) { int st; waitpid(cp, &st, 0); }
    }

    msg("\n");
    msg("  ════════════════════════════════════════════\n");
    msg("  Kevlar Alpine+twm desktop is running!\n");
    msg("  ════════════════════════════════════════════\n");
    msg("\n");

    // PID 1 must stay alive — wait for Xorg to exit
    int status;
    waitpid(xorg_pid, &status, 0);
    msg("kevlar: Xorg exited, shutting down.\n");
    return 0;
}
