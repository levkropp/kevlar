// Comprehensive Alpine smoke test: validates Kevlar as a drop-in Linux replacement.
// Boots Alpine ext2, runs 8 test phases covering filesystem, shell, processes,
// packages, networking, build tools, system info, and stress scenarios.
// Each test emits TEST_PASS/TEST_FAIL for machine-readable collection.
#define _GNU_SOURCE
#include <arpa/inet.h>
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>
#include <time.h>

// ─── Test Accounting ─────────────────────────────────────────────────────────

static int g_pass, g_fail, g_skip;

static void pass(const char *name) {
    printf("TEST_PASS %s\n", name);
    g_pass++;
}

static void fail(const char *name, const char *detail) {
    if (detail)
        printf("TEST_FAIL %s (%s)\n", name, detail);
    else
        printf("TEST_FAIL %s\n", name);
    g_fail++;
}

static void skip(const char *name, const char *reason) {
    printf("TEST_SKIP %s (%s)\n", name, reason);
    g_skip++;
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

static int file_exists(const char *path) {
    struct stat st;
    return stat(path, &st) == 0;
}

// Run a command in the Alpine chroot, capture stdout+stderr. Returns exit code.
// -1 = error, -2 = timeout.
static int chroot_exec(const char *rootdir, const char *const argv[],
                       char *out, int outsz, int timeout_ms) {
    int pipefd[2];
    if (pipe(pipefd) < 0) return -1;

    pid_t pid = fork();
    if (pid < 0) { close(pipefd[0]); close(pipefd[1]); return -1; }

    if (pid == 0) {
        close(pipefd[0]);
        dup2(pipefd[1], STDOUT_FILENO);
        dup2(pipefd[1], STDERR_FILENO);
        close(pipefd[1]);
        if (chroot(rootdir) < 0) _exit(126);
        if (chdir("/") < 0) _exit(126);
        // Set up minimal environment
        char *envp[] = {
            "PATH=/usr/sbin:/usr/bin:/sbin:/bin",
            "HOME=/root",
            "TERM=vt100",
            NULL,
        };
        execve(argv[0], (char *const *)argv, envp);
        _exit(127);
    }

    close(pipefd[1]);
    int pos = 0;
    struct pollfd pfd = { .fd = pipefd[0], .events = POLLIN };
    while (pos < outsz - 1) {
        int r = poll(&pfd, 1, timeout_ms);
        if (r <= 0) break;
        ssize_t n = read(pipefd[0], out + pos, outsz - 1 - pos);
        if (n <= 0) break;
        pos += n;
    }
    out[pos] = '\0';
    close(pipefd[0]);

    int status = 0, waited = 0;
    for (int i = 0; i < timeout_ms / 100 + 1; i++) {
        pid_t w = waitpid(pid, &status, WNOHANG);
        if (w > 0) { waited = 1; break; }
        if (w < 0) break;
        usleep(100000);
    }
    if (!waited) {
        kill(pid, SIGKILL);
        waitpid(pid, &status, 0);
        return -2;
    }
    if (WIFEXITED(status)) return WEXITSTATUS(status);
    if (WIFSIGNALED(status)) return 128 + WTERMSIG(status);
    return -1;
}

// Convenience: run /bin/sh -c "cmd" in chroot
static int sh_exec(const char *rootdir, const char *cmd,
                   char *out, int outsz, int timeout_ms) {
    const char *argv[] = { "/bin/sh", "-c", cmd, NULL };
    return chroot_exec(rootdir, argv, out, outsz, timeout_ms);
}

#define ROOT "/mnt"
#define BUF_SZ 8192
static char g_buf[BUF_SZ];
static char g_detail[256];

// ─── Phase 1: Boot & Mount ──────────────────────────────────────────────────

static int phase1_boot(void) {
    printf("\n=== PHASE 1: Boot & Mount ===\n");
    int ok = 1;

    mkdir("/mnt", 0755);
    if (mount("/dev/vda", ROOT, "ext2", 0, NULL) == 0) {
        pass("p1_mount_ext2");
    } else {
        snprintf(g_detail, sizeof(g_detail), "errno=%d", errno);
        fail("p1_mount_ext2", g_detail);
        return 0;
    }

    // Mount virtual filesystems
    mkdir(ROOT "/proc", 0755);
    mount("proc", ROOT "/proc", "proc", 0, NULL);
    mkdir(ROOT "/sys", 0755);
    mount("sysfs", ROOT "/sys", "sysfs", 0, NULL);
    mkdir(ROOT "/dev", 0755);
    mount("devtmpfs", ROOT "/dev", "devtmpfs", 0, NULL);
    mkdir(ROOT "/dev/pts", 0755);
    mkdir(ROOT "/dev/shm", 0755);
    mkdir(ROOT "/tmp", 01777);
    mount("tmpfs", ROOT "/tmp", "tmpfs", 0, NULL);
    mkdir(ROOT "/run", 0755);
    mount("tmpfs", ROOT "/run", "tmpfs", 0, NULL);

    // Copy resolv.conf for DNS
    {
        int fd = open(ROOT "/etc/resolv.conf", O_WRONLY | O_CREAT | O_TRUNC, 0644);
        if (fd >= 0) {
            const char *dns = "nameserver 10.0.2.3\n";
            write(fd, dns, strlen(dns));
            close(fd);
        }
    }

    // Verify critical Alpine paths
    struct { const char *path; const char *name; } checks[] = {
        { ROOT "/bin/busybox",              "p1_busybox" },
        { ROOT "/lib/ld-musl-x86_64.so.1", "p1_musl_ld" },
        { ROOT "/etc/alpine-release",       "p1_alpine_release" },
        { ROOT "/etc/apk/repositories",     "p1_apk_repos" },
    };
    for (int i = 0; i < 4; i++) {
        if (file_exists(checks[i].path))
            pass(checks[i].name);
        else {
            fail(checks[i].name, "not found");
            ok = 0;
        }
    }

    // Read Alpine version
    {
        int fd = open(ROOT "/etc/alpine-release", O_RDONLY);
        if (fd >= 0) {
            char ver[32];
            ssize_t n = read(fd, ver, sizeof(ver) - 1);
            close(fd);
            if (n > 0) {
                ver[n] = '\0';
                // Strip trailing newline
                if (n > 0 && ver[n-1] == '\n') ver[n-1] = '\0';
                printf("  Alpine version: %s\n", ver);
            }
        }
    }

    return ok;
}

// ─── Phase 2: Filesystem Operations ─────────────────────────────────────────

static void phase2_filesystem(void) {
    printf("\n=== PHASE 2: Filesystem Operations ===\n");

    // Create, write, read, verify
    {
        int fd = open(ROOT "/tmp/smoke_test", O_CREAT | O_WRONLY | O_TRUNC, 0644);
        if (fd >= 0) {
            write(fd, "hello kevlar\n", 13);
            close(fd);
            fd = open(ROOT "/tmp/smoke_test", O_RDONLY);
            char buf[64];
            ssize_t n = read(fd, buf, sizeof(buf) - 1);
            close(fd);
            buf[n > 0 ? n : 0] = '\0';
            if (strcmp(buf, "hello kevlar\n") == 0)
                pass("p2_write_read");
            else
                fail("p2_write_read", "data mismatch");
        } else {
            fail("p2_write_read", "open failed");
        }
    }

    // mkdir + rmdir
    if (mkdir(ROOT "/tmp/smoke_dir", 0755) == 0 &&
        file_exists(ROOT "/tmp/smoke_dir") &&
        rmdir(ROOT "/tmp/smoke_dir") == 0)
        pass("p2_mkdir_rmdir");
    else
        fail("p2_mkdir_rmdir", NULL);

    // Symlink + readlink
    {
        unlink(ROOT "/tmp/smoke_link");
        if (symlink("smoke_test", ROOT "/tmp/smoke_link") == 0) {
            char lnk[128];
            ssize_t n = readlink(ROOT "/tmp/smoke_link", lnk, sizeof(lnk) - 1);
            if (n > 0) {
                lnk[n] = '\0';
                if (strcmp(lnk, "smoke_test") == 0)
                    pass("p2_symlink_readlink");
                else
                    fail("p2_symlink_readlink", "wrong target");
            } else {
                fail("p2_symlink_readlink", "readlink failed");
            }
        } else {
            fail("p2_symlink_readlink", "symlink failed");
        }
        unlink(ROOT "/tmp/smoke_link");
    }

    // Hard link
    {
        unlink(ROOT "/tmp/smoke_hardlink");
        if (link(ROOT "/tmp/smoke_test", ROOT "/tmp/smoke_hardlink") == 0) {
            struct stat s1, s2;
            stat(ROOT "/tmp/smoke_test", &s1);
            stat(ROOT "/tmp/smoke_hardlink", &s2);
            if (s1.st_ino == s2.st_ino && s1.st_nlink == 2)
                pass("p2_hardlink");
            else
                fail("p2_hardlink", "ino/nlink mismatch");
        } else {
            snprintf(g_detail, sizeof(g_detail), "errno=%d", errno);
            fail("p2_hardlink", g_detail);
        }
        unlink(ROOT "/tmp/smoke_hardlink");
    }

    // Rename
    {
        if (rename(ROOT "/tmp/smoke_test", ROOT "/tmp/smoke_renamed") == 0 &&
            file_exists(ROOT "/tmp/smoke_renamed") &&
            !file_exists(ROOT "/tmp/smoke_test"))
            pass("p2_rename");
        else
            fail("p2_rename", NULL);
        unlink(ROOT "/tmp/smoke_renamed");
    }

    // chmod + verify
    {
        int fd = open(ROOT "/tmp/smoke_perm", O_CREAT | O_WRONLY | O_TRUNC, 0644);
        if (fd >= 0) {
            close(fd);
            chmod(ROOT "/tmp/smoke_perm", 0755);
            struct stat st;
            stat(ROOT "/tmp/smoke_perm", &st);
            if ((st.st_mode & 0777) == 0755)
                pass("p2_chmod");
            else {
                snprintf(g_detail, sizeof(g_detail), "mode=0%o", st.st_mode & 0777);
                fail("p2_chmod", g_detail);
            }
            unlink(ROOT "/tmp/smoke_perm");
        } else {
            fail("p2_chmod", "open failed");
        }
    }

    // Large file: write 1MB, verify size
    {
        int fd = open(ROOT "/tmp/smoke_large", O_CREAT | O_WRONLY | O_TRUNC, 0644);
        if (fd >= 0) {
            char block[4096];
            memset(block, 'X', sizeof(block));
            int total = 0;
            for (int i = 0; i < 256; i++) { // 256 * 4K = 1MB
                ssize_t n = write(fd, block, sizeof(block));
                if (n > 0) total += n;
                else break;
            }
            close(fd);
            struct stat st;
            stat(ROOT "/tmp/smoke_large", &st);
            if (total == 1048576 && st.st_size == 1048576)
                pass("p2_large_file_1mb");
            else {
                snprintf(g_detail, sizeof(g_detail), "wrote=%d size=%ld",
                         total, (long)st.st_size);
                fail("p2_large_file_1mb", g_detail);
            }
            unlink(ROOT "/tmp/smoke_large");
        } else {
            fail("p2_large_file_1mb", "open failed");
        }
    }

    // Deep directory: create 10 levels
    {
        char path[256];
        strcpy(path, ROOT "/tmp");
        int ok = 1;
        for (int i = 0; i < 10; i++) {
            char level[16];
            snprintf(level, sizeof(level), "/d%d", i);
            strcat(path, level);
            if (mkdir(path, 0755) != 0 && errno != EEXIST) { ok = 0; break; }
        }
        // Write a file at the bottom
        if (ok) {
            strcat(path, "/deep_file");
            int fd = open(path, O_CREAT | O_WRONLY | O_TRUNC, 0644);
            if (fd >= 0) {
                write(fd, "deep\n", 5);
                close(fd);
                pass("p2_deep_directory");
            } else {
                fail("p2_deep_directory", "write at depth failed");
            }
        } else {
            fail("p2_deep_directory", "mkdir failed");
        }
        // Cleanup (best effort)
        strcpy(path, ROOT "/tmp/d0/d1/d2/d3/d4/d5/d6/d7/d8/d9/deep_file");
        unlink(path);
        for (int i = 9; i >= 0; i--) {
            char *last = strrchr(path, '/');
            if (last) { *last = '\0'; rmdir(path); }
        }
    }

    // Directory listing: create files, readdir, count
    {
        mkdir(ROOT "/tmp/smoke_listing", 0755);
        for (int i = 0; i < 20; i++) {
            char name[64];
            snprintf(name, sizeof(name), ROOT "/tmp/smoke_listing/file_%02d", i);
            int fd = open(name, O_CREAT | O_WRONLY | O_TRUNC, 0644);
            if (fd >= 0) close(fd);
        }
        DIR *d = opendir(ROOT "/tmp/smoke_listing");
        int count = 0;
        if (d) {
            struct dirent *ent;
            while ((ent = readdir(d)) != NULL) {
                if (ent->d_name[0] != '.') count++;
            }
            closedir(d);
        }
        if (count == 20)
            pass("p2_readdir_20_files");
        else {
            snprintf(g_detail, sizeof(g_detail), "count=%d expected=20", count);
            fail("p2_readdir_20_files", g_detail);
        }
        // Cleanup
        for (int i = 0; i < 20; i++) {
            char name[64];
            snprintf(name, sizeof(name), ROOT "/tmp/smoke_listing/file_%02d", i);
            unlink(name);
        }
        rmdir(ROOT "/tmp/smoke_listing");
    }
}

// ─── Phase 3: Shell & Utilities ─────────────────────────────────────────────

static void phase3_shell(void) {
    printf("\n=== PHASE 3: Shell & Utilities ===\n");

    // Basic shell execution
    int rc = sh_exec(ROOT, "echo hello", g_buf, BUF_SZ, 5000);
    if (rc == 0 && strstr(g_buf, "hello"))
        pass("p3_echo");
    else
        fail("p3_echo", g_buf);

    // Pipe chain: ls | grep | wc
    rc = sh_exec(ROOT, "ls /bin | grep busybox | wc -l", g_buf, BUF_SZ, 5000);
    if (rc == 0 && atoi(g_buf) >= 1)
        pass("p3_pipe_chain");
    else {
        snprintf(g_detail, sizeof(g_detail), "rc=%d out='%.40s'", rc, g_buf);
        fail("p3_pipe_chain", g_detail);
    }

    // Command substitution
    rc = sh_exec(ROOT, "echo $(cat /etc/alpine-release)", g_buf, BUF_SZ, 5000);
    if (rc == 0 && (strstr(g_buf, "3.2") || strstr(g_buf, "3.1")))
        pass("p3_command_substitution");
    else {
        snprintf(g_detail, sizeof(g_detail), "rc=%d out='%.40s'", rc, g_buf);
        fail("p3_command_substitution", g_detail);
    }

    // Redirect: write to file, read back
    rc = sh_exec(ROOT, "echo redirect_test > /tmp/redir && cat /tmp/redir",
                 g_buf, BUF_SZ, 5000);
    if (rc == 0 && strstr(g_buf, "redirect_test"))
        pass("p3_redirect");
    else
        fail("p3_redirect", g_buf);

    // Append redirect
    rc = sh_exec(ROOT, "echo line1 > /tmp/append && echo line2 >> /tmp/append && wc -l < /tmp/append",
                 g_buf, BUF_SZ, 5000);
    if (rc == 0 && atoi(g_buf) == 2)
        pass("p3_append_redirect");
    else {
        snprintf(g_detail, sizeof(g_detail), "rc=%d out='%.40s'", rc, g_buf);
        fail("p3_append_redirect", g_detail);
    }

    // For loop
    rc = sh_exec(ROOT, "sum=0; for i in 1 2 3 4 5; do sum=$((sum + i)); done; echo $sum",
                 g_buf, BUF_SZ, 5000);
    if (rc == 0 && atoi(g_buf) == 15)
        pass("p3_for_loop");
    else {
        snprintf(g_detail, sizeof(g_detail), "rc=%d out='%.40s'", rc, g_buf);
        fail("p3_for_loop", g_detail);
    }

    // While loop
    rc = sh_exec(ROOT, "i=0; while [ $i -lt 5 ]; do i=$((i+1)); done; echo $i",
                 g_buf, BUF_SZ, 5000);
    if (rc == 0 && atoi(g_buf) == 5)
        pass("p3_while_loop");
    else {
        snprintf(g_detail, sizeof(g_detail), "rc=%d out='%.40s'", rc, g_buf);
        fail("p3_while_loop", g_detail);
    }

    // Conditionals (if/then/else)
    rc = sh_exec(ROOT, "if [ -f /etc/alpine-release ]; then echo yes; else echo no; fi",
                 g_buf, BUF_SZ, 5000);
    if (rc == 0 && strstr(g_buf, "yes"))
        pass("p3_conditional");
    else
        fail("p3_conditional", g_buf);

    // grep with regex
    rc = sh_exec(ROOT, "echo 'foo123bar' | grep -o '[0-9]\\+'",
                 g_buf, BUF_SZ, 5000);
    if (rc == 0 && strstr(g_buf, "123"))
        pass("p3_grep_regex");
    else {
        snprintf(g_detail, sizeof(g_detail), "rc=%d out='%.40s'", rc, g_buf);
        fail("p3_grep_regex", g_detail);
    }

    // sed substitution
    rc = sh_exec(ROOT, "echo 'hello world' | sed 's/world/kevlar/'",
                 g_buf, BUF_SZ, 5000);
    if (rc == 0 && strstr(g_buf, "hello kevlar"))
        pass("p3_sed");
    else
        fail("p3_sed", g_buf);

    // awk field extraction
    rc = sh_exec(ROOT, "echo 'one two three' | awk '{print $2}'",
                 g_buf, BUF_SZ, 5000);
    if (rc == 0 && strstr(g_buf, "two"))
        pass("p3_awk");
    else
        fail("p3_awk", g_buf);

    // sort + uniq
    rc = sh_exec(ROOT, "printf 'b\\na\\nc\\na\\nb\\n' | sort | uniq | wc -l",
                 g_buf, BUF_SZ, 5000);
    if (rc == 0 && atoi(g_buf) == 3)
        pass("p3_sort_uniq");
    else {
        snprintf(g_detail, sizeof(g_detail), "rc=%d out='%.40s'", rc, g_buf);
        fail("p3_sort_uniq", g_detail);
    }

    // find
    rc = sh_exec(ROOT, "find /etc -name 'alpine-release' -type f 2>/dev/null | head -1",
                 g_buf, BUF_SZ, 10000);
    if (rc == 0 && strstr(g_buf, "alpine-release"))
        pass("p3_find");
    else {
        snprintf(g_detail, sizeof(g_detail), "rc=%d out='%.40s'", rc, g_buf);
        fail("p3_find", g_detail);
    }

    // tar + gzip round-trip
    rc = sh_exec(ROOT,
        "mkdir -p /tmp/tartest && echo content > /tmp/tartest/f1 && "
        "tar czf /tmp/tartest.tar.gz -C /tmp tartest && "
        "rm -rf /tmp/tartest && "
        "tar xzf /tmp/tartest.tar.gz -C /tmp && "
        "cat /tmp/tartest/f1",
        g_buf, BUF_SZ, 10000);
    if (rc == 0 && strstr(g_buf, "content"))
        pass("p3_tar_gzip");
    else {
        snprintf(g_detail, sizeof(g_detail), "rc=%d out='%.40s'", rc, g_buf);
        fail("p3_tar_gzip", g_detail);
    }

    // head + tail
    rc = sh_exec(ROOT,
        "seq 1 10 | head -3 | tail -1",
        g_buf, BUF_SZ, 5000);
    if (rc == 0 && atoi(g_buf) == 3)
        pass("p3_head_tail");
    else {
        snprintf(g_detail, sizeof(g_detail), "rc=%d out='%.40s'", rc, g_buf);
        fail("p3_head_tail", g_detail);
    }

    // xargs
    rc = sh_exec(ROOT,
        "echo 'a b c' | xargs -n1 echo | wc -l",
        g_buf, BUF_SZ, 5000);
    if (rc == 0 && atoi(g_buf) == 3)
        pass("p3_xargs");
    else {
        snprintf(g_detail, sizeof(g_detail), "rc=%d out='%.40s'", rc, g_buf);
        fail("p3_xargs", g_detail);
    }

    // cut + tr
    rc = sh_exec(ROOT,
        "echo 'foo:bar:baz' | cut -d: -f2 | tr 'a-z' 'A-Z'",
        g_buf, BUF_SZ, 5000);
    if (rc == 0 && strstr(g_buf, "BAR"))
        pass("p3_cut_tr");
    else
        fail("p3_cut_tr", g_buf);

    // expr / arithmetic
    rc = sh_exec(ROOT, "echo $((17 * 3 + 2))", g_buf, BUF_SZ, 5000);
    if (rc == 0 && atoi(g_buf) == 53)
        pass("p3_arithmetic");
    else {
        snprintf(g_detail, sizeof(g_detail), "rc=%d out='%.40s'", rc, g_buf);
        fail("p3_arithmetic", g_detail);
    }

    // Exit status
    rc = sh_exec(ROOT, "false; echo $?", g_buf, BUF_SZ, 5000);
    if (rc == 0 && atoi(g_buf) == 1)
        pass("p3_exit_status");
    else {
        snprintf(g_detail, sizeof(g_detail), "rc=%d out='%.40s'", rc, g_buf);
        fail("p3_exit_status", g_detail);
    }

    // Here-document (via echo to simulate)
    rc = sh_exec(ROOT,
        "cat <<'ENDOFHERE'\nhello from heredoc\nENDOFHERE\n",
        g_buf, BUF_SZ, 5000);
    if (rc == 0 && strstr(g_buf, "hello from heredoc"))
        pass("p3_heredoc");
    else {
        snprintf(g_detail, sizeof(g_detail), "rc=%d out='%.60s'", rc, g_buf);
        fail("p3_heredoc", g_detail);
    }
}

// ─── Phase 4: Process Management ────────────────────────────────────────────

static void phase4_processes(void) {
    printf("\n=== PHASE 4: Process Management ===\n");

    // fork + exec + wait
    {
        pid_t pid = fork();
        if (pid == 0) _exit(42);
        int status;
        waitpid(pid, &status, 0);
        if (WIFEXITED(status) && WEXITSTATUS(status) == 42)
            pass("p4_fork_exit_status");
        else
            fail("p4_fork_exit_status", "wrong exit status");
    }

    // Pipe between parent and child
    {
        int pfd[2];
        pipe(pfd);
        pid_t pid = fork();
        if (pid == 0) {
            close(pfd[0]);
            write(pfd[1], "pipe_ok", 7);
            close(pfd[1]);
            _exit(0);
        }
        close(pfd[1]);
        char buf[32] = {0};
        read(pfd[0], buf, sizeof(buf) - 1);
        close(pfd[0]);
        waitpid(pid, NULL, 0);
        if (strcmp(buf, "pipe_ok") == 0)
            pass("p4_pipe_parent_child");
        else
            fail("p4_pipe_parent_child", buf);
    }

    // SIGTERM delivery
    {
        pid_t pid = fork();
        if (pid == 0) { pause(); _exit(0); }
        usleep(50000);
        kill(pid, SIGTERM);
        int status;
        waitpid(pid, &status, 0);
        if (WIFSIGNALED(status) && WTERMSIG(status) == SIGTERM)
            pass("p4_sigterm");
        else {
            snprintf(g_detail, sizeof(g_detail), "status=0x%x", status);
            fail("p4_sigterm", g_detail);
        }
    }

    // SIGKILL delivery
    {
        pid_t pid = fork();
        if (pid == 0) {
            // Block all signals — SIGKILL must still work
            sigset_t all;
            sigfillset(&all);
            sigprocmask(SIG_BLOCK, &all, NULL);
            while (1) pause();
        }
        usleep(50000);
        kill(pid, SIGKILL);
        int status;
        pid_t r = waitpid(pid, &status, 0);
        if (r == pid && WIFSIGNALED(status) && WTERMSIG(status) == SIGKILL)
            pass("p4_sigkill");
        else {
            snprintf(g_detail, sizeof(g_detail), "r=%d status=0x%x", r, status);
            fail("p4_sigkill", g_detail);
        }
    }

    // SIGTSTP + SIGCONT
    {
        pid_t pid = fork();
        if (pid == 0) { while (1) usleep(10000); }
        usleep(50000);
        kill(pid, SIGTSTP);
        int status;
        pid_t r = waitpid(pid, &status, WUNTRACED);
        if (r == pid && WIFSTOPPED(status)) {
            // Resume and kill
            kill(pid, SIGCONT);
            usleep(50000);
            kill(pid, SIGTERM);
            waitpid(pid, &status, 0);
            if (WIFSIGNALED(status))
                pass("p4_sigtstp_sigcont");
            else {
                snprintf(g_detail, sizeof(g_detail), "after CONT status=0x%x", status);
                fail("p4_sigtstp_sigcont", g_detail);
            }
        } else {
            snprintf(g_detail, sizeof(g_detail), "stop: r=%d status=0x%x", r, status);
            fail("p4_sigtstp_sigcont", g_detail);
            kill(pid, SIGKILL);
            waitpid(pid, NULL, 0);
        }
    }

    // SIGINT to foreground child
    {
        pid_t pid = fork();
        if (pid == 0) {
            // Become own process group
            setpgid(0, 0);
            pause();
            _exit(0);
        }
        usleep(50000);
        kill(pid, SIGINT);
        int status;
        waitpid(pid, &status, 0);
        if (WIFSIGNALED(status) && WTERMSIG(status) == SIGINT)
            pass("p4_sigint");
        else {
            snprintf(g_detail, sizeof(g_detail), "status=0x%x", status);
            fail("p4_sigint", g_detail);
        }
    }

    // Multiple children + waitpid
    {
        pid_t pids[5];
        for (int i = 0; i < 5; i++) {
            pids[i] = fork();
            if (pids[i] == 0) _exit(i + 10);
        }
        int ok = 1;
        for (int i = 0; i < 5; i++) {
            int status;
            pid_t r = waitpid(pids[i], &status, 0);
            if (r != pids[i] || !WIFEXITED(status) ||
                WEXITSTATUS(status) != i + 10)
                ok = 0;
        }
        if (ok)
            pass("p4_multi_child_wait");
        else
            fail("p4_multi_child_wait", "some children had wrong status");
    }

    // Background process via shell (&)
    {
        int rc = sh_exec(ROOT,
            "sleep 0 & BGPID=$!; wait $BGPID; echo exit=$?",
            g_buf, BUF_SZ, 5000);
        if (rc == 0 && strstr(g_buf, "exit=0"))
            pass("p4_background_wait");
        else {
            snprintf(g_detail, sizeof(g_detail), "rc=%d out='%.40s'", rc, g_buf);
            fail("p4_background_wait", g_detail);
        }
    }

    // Process group kill
    {
        int sync_pipe[2];
        pipe(sync_pipe);
        pid_t pid = fork();
        if (pid == 0) {
            close(sync_pipe[0]);
            setpgid(0, 0);
            pid_t child = fork();
            if (child == 0) {
                close(sync_pipe[1]);
                while (1) usleep(10000);
            }
            // Signal parent that process group is ready
            write(sync_pipe[1], "R", 1);
            close(sync_pipe[1]);
            while (1) usleep(10000);
        }
        close(sync_pipe[1]);
        // Wait for child to signal readiness (with 2s timeout)
        struct pollfd spfd = { .fd = sync_pipe[0], .events = POLLIN };
        poll(&spfd, 1, 2000);
        close(sync_pipe[0]);
        // Kill the process group
        kill(-pid, SIGKILL);
        int status;
        // Use WNOHANG loop with timeout instead of blocking waitpid
        int waited = 0;
        for (int i = 0; i < 30; i++) { // 3 seconds max
            pid_t r = waitpid(pid, &status, WNOHANG);
            if (r > 0) { waited = 1; break; }
            usleep(100000);
        }
        if (waited && WIFSIGNALED(status))
            pass("p4_pgid_kill");
        else {
            if (!waited) {
                kill(pid, SIGKILL);
                waitpid(pid, &status, 0);
            }
            snprintf(g_detail, sizeof(g_detail), "waited=%d status=0x%x", waited, status);
            fail("p4_pgid_kill", g_detail);
        }
    }
}

// ─── Phase 5: System Info ───────────────────────────────────────────────────

static void phase5_sysinfo(void) {
    printf("\n=== PHASE 5: System Info ===\n");

    // uname
    int rc = sh_exec(ROOT, "uname -s", g_buf, BUF_SZ, 5000);
    if (rc == 0 && strstr(g_buf, "Linux"))
        pass("p5_uname");
    else {
        snprintf(g_detail, sizeof(g_detail), "rc=%d out='%.40s'", rc, g_buf);
        fail("p5_uname", g_detail);
    }

    // /proc/version
    rc = sh_exec(ROOT, "cat /proc/version", g_buf, BUF_SZ, 5000);
    if (rc == 0 && strlen(g_buf) > 10)
        pass("p5_proc_version");
    else
        fail("p5_proc_version", "missing or empty");

    // /proc/meminfo
    rc = sh_exec(ROOT, "grep MemTotal /proc/meminfo", g_buf, BUF_SZ, 5000);
    if (rc == 0 && strstr(g_buf, "MemTotal"))
        pass("p5_proc_meminfo");
    else
        fail("p5_proc_meminfo", "no MemTotal");

    // /proc/cpuinfo
    rc = sh_exec(ROOT, "grep -c processor /proc/cpuinfo", g_buf, BUF_SZ, 5000);
    if (rc == 0 && atoi(g_buf) >= 1)
        pass("p5_proc_cpuinfo");
    else {
        snprintf(g_detail, sizeof(g_detail), "rc=%d out='%.40s'", rc, g_buf);
        fail("p5_proc_cpuinfo", g_detail);
    }

    // /proc/self/maps
    rc = sh_exec(ROOT, "wc -l < /proc/self/maps", g_buf, BUF_SZ, 5000);
    if (rc == 0 && atoi(g_buf) >= 1)
        pass("p5_proc_self_maps");
    else
        fail("p5_proc_self_maps", g_buf);

    // /proc/self/status
    rc = sh_exec(ROOT, "grep Pid /proc/self/status", g_buf, BUF_SZ, 5000);
    if (rc == 0 && strstr(g_buf, "Pid"))
        pass("p5_proc_self_status");
    else
        fail("p5_proc_self_status", g_buf);

    // hostname
    rc = sh_exec(ROOT, "hostname", g_buf, BUF_SZ, 5000);
    if (rc == 0 && strlen(g_buf) > 0)
        pass("p5_hostname");
    else
        fail("p5_hostname", "empty");

    // uptime
    rc = sh_exec(ROOT, "cat /proc/uptime", g_buf, BUF_SZ, 5000);
    if (rc == 0 && strlen(g_buf) > 3)
        pass("p5_proc_uptime");
    else
        fail("p5_proc_uptime", g_buf);

    // /proc/mounts
    rc = sh_exec(ROOT, "grep -c ' ' /proc/mounts", g_buf, BUF_SZ, 5000);
    if (rc == 0 && atoi(g_buf) >= 1)
        pass("p5_proc_mounts");
    else
        fail("p5_proc_mounts", g_buf);

    // /proc/self/fd (verify fds visible)
    rc = sh_exec(ROOT, "ls /proc/self/fd | wc -l", g_buf, BUF_SZ, 5000);
    if (rc == 0 && atoi(g_buf) >= 3) // stdin, stdout, stderr at minimum
        pass("p5_proc_self_fd");
    else {
        snprintf(g_detail, sizeof(g_detail), "rc=%d out='%.40s'", rc, g_buf);
        fail("p5_proc_self_fd", g_detail);
    }
}

// ─── Phase 6: Networking ────────────────────────────────────────────────────

static void phase6_network(void) {
    printf("\n=== PHASE 6: Networking ===\n");

    // DNS resolution via UDP
    {
        int fd = socket(AF_INET, SOCK_DGRAM, 0);
        if (fd < 0) {
            fail("p6_dns_socket", "socket failed");
            return;
        }
        struct sockaddr_in dns = {
            .sin_family = AF_INET,
            .sin_port = htons(53),
        };
        inet_aton("10.0.2.3", &dns.sin_addr);
        connect(fd, (struct sockaddr *)&dns, sizeof(dns));

        // Query for dl-cdn.alpinelinux.org
        unsigned char query[] = {
            0xAB, 0xCD, 0x01, 0x00, 0x00, 0x01,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            6, 'd','l','-','c','d','n',
            11, 'a','l','p','i','n','e','l','i','n','u','x',
            3, 'o','r','g', 0,
            0x00, 0x01, 0x00, 0x01,
        };
        send(fd, query, sizeof(query), 0);

        struct pollfd pfd = { .fd = fd, .events = POLLIN };
        int r = poll(&pfd, 1, 5000);
        if (r > 0) {
            unsigned char resp[512];
            ssize_t n = recv(fd, resp, sizeof(resp), 0);
            int ancount = (n >= 12) ? ((resp[6] << 8) | resp[7]) : 0;
            if (ancount > 0)
                pass("p6_dns_resolve");
            else
                fail("p6_dns_resolve", "ANCOUNT=0");
        } else {
            fail("p6_dns_resolve", "timeout");
        }
        close(fd);
    }

    // TCP HTTP: raw HTTP GET to Alpine CDN (avoids wget timeout issues)
    {
        struct sockaddr_in cdn = {
            .sin_family = AF_INET,
            .sin_port = htons(80),
        };
        // Use resolved DNS IP or fall back to known CDN IP
        // (DNS test already resolved dl-cdn.alpinelinux.org)
        int fd = socket(AF_INET, SOCK_STREAM, 0);
        if (fd >= 0) {
            inet_aton("146.75.118.132", &cdn.sin_addr); // Fastly CDN fallback
            // Try raw DNS first for the real IP
            int rc = sh_exec(ROOT, "nslookup dl-cdn.alpinelinux.org 2>/dev/null | grep -m1 'Address:' | tail -1 | awk '{print $NF}'",
                             g_buf, BUF_SZ, 5000);
            if (rc == 0 && g_buf[0] >= '0' && g_buf[0] <= '9') {
                char *nl = strchr(g_buf, '\n');
                if (nl) *nl = '\0';
                inet_aton(g_buf, &cdn.sin_addr);
            }

            struct pollfd cpfd = { .fd = fd, .events = POLLOUT };
            // Non-blocking connect with timeout
            int flags = fcntl(fd, F_GETFL, 0);
            fcntl(fd, F_SETFL, flags | O_NONBLOCK);
            connect(fd, (struct sockaddr *)&cdn, sizeof(cdn));
            int r = poll(&cpfd, 1, 10000);
            fcntl(fd, F_SETFL, flags); // restore blocking

            if (r > 0) {
                const char *req =
                    "GET /alpine/v3.21/main/x86_64/APKINDEX.tar.gz HTTP/1.0\r\n"
                    "Host: dl-cdn.alpinelinux.org\r\n\r\n";
                write(fd, req, strlen(req));

                struct pollfd rpfd = { .fd = fd, .events = POLLIN };
                r = poll(&rpfd, 1, 10000);
                if (r > 0) {
                    char hdr[512] = {0};
                    ssize_t n = read(fd, hdr, sizeof(hdr) - 1);
                    if (n > 0 && strstr(hdr, "200 OK"))
                        pass("p6_http_get");
                    else if (n > 0 && (strstr(hdr, "301") || strstr(hdr, "302")))
                        pass("p6_http_get"); // redirect is also fine
                    else {
                        hdr[n > 60 ? 60 : (n > 0 ? n : 0)] = '\0';
                        snprintf(g_detail, sizeof(g_detail), "n=%zd hdr='%.50s'", n, hdr);
                        fail("p6_http_get", g_detail);
                    }
                } else {
                    fail("p6_http_get", "read timeout");
                }
            } else {
                fail("p6_http_get", "connect timeout");
            }
            close(fd);
        } else {
            fail("p6_http_get", "socket failed");
        }
    }

    // Unix domain socket pair
    {
        int sv[2];
        if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0) {
            write(sv[0], "unix_ok", 7);
            char buf[16] = {0};
            read(sv[1], buf, sizeof(buf) - 1);
            close(sv[0]);
            close(sv[1]);
            if (strcmp(buf, "unix_ok") == 0)
                pass("p6_unix_socketpair");
            else
                fail("p6_unix_socketpair", buf);
        } else {
            fail("p6_unix_socketpair", "socketpair failed");
        }
    }

    // TCP loopback: server on 127.0.0.1, client connects, exchange data.
    {
        int sfd = socket(AF_INET, SOCK_STREAM, 0);
        if (sfd < 0) {
            fail("p6_tcp_loopback", "socket failed");
        } else {
            struct sockaddr_in addr = {
                .sin_family = AF_INET,
                .sin_port = htons(18080),
            };
            inet_aton("127.0.0.1", &addr.sin_addr);
            int one = 1;
            setsockopt(sfd, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one));
            if (bind(sfd, (struct sockaddr *)&addr, sizeof(addr)) < 0 ||
                listen(sfd, 1) < 0) {
                snprintf(g_detail, sizeof(g_detail), "bind/listen errno=%d", errno);
                fail("p6_tcp_loopback", g_detail);
                close(sfd);
            } else {
                pid_t pid = fork();
                if (pid == 0) {
                    close(sfd);
                    int cfd = socket(AF_INET, SOCK_STREAM, 0);
                    if (connect(cfd, (struct sockaddr *)&addr, sizeof(addr)) == 0)
                        write(cfd, "loopback", 8);
                    close(cfd);
                    _exit(0);
                }
                struct pollfd apfd = { .fd = sfd, .events = POLLIN };
                int r = poll(&apfd, 1, 5000);
                if (r > 0) {
                    int afd = accept(sfd, NULL, NULL);
                    if (afd >= 0) {
                        char buf[16] = {0};
                        struct pollfd rpfd = { .fd = afd, .events = POLLIN };
                        if (poll(&rpfd, 1, 3000) > 0)
                            read(afd, buf, sizeof(buf) - 1);
                        close(afd);
                        if (strcmp(buf, "loopback") == 0)
                            pass("p6_tcp_loopback");
                        else {
                            snprintf(g_detail, sizeof(g_detail), "got='%s'", buf);
                            fail("p6_tcp_loopback", g_detail);
                        }
                    } else {
                        fail("p6_tcp_loopback", "accept failed");
                    }
                } else {
                    fail("p6_tcp_loopback", "accept timeout");
                }
                close(sfd);
                kill(pid, SIGKILL);
                waitpid(pid, NULL, 0);
            }
        }
    }
}

// ─── Phase 7: Package Management ────────────────────────────────────────────

static void phase7_packages(int net_ok) {
    printf("\n=== PHASE 7: Package Management ===\n");

    if (!net_ok) {
        skip("p7_apk_update", "network unavailable");
        skip("p7_apk_add_file", "network unavailable");
        skip("p7_installed_binary", "network unavailable");
        skip("p7_apk_info", "network unavailable");
        return;
    }

    // Copy apk.static from initramfs if available
    {
        int src = open("/bin/apk.static", O_RDONLY);
        if (src >= 0) {
            mkdir(ROOT "/usr/sbin", 0755);
            int dst = open(ROOT "/sbin/apk", O_WRONLY | O_CREAT | O_TRUNC, 0755);
            if (dst < 0)
                dst = open(ROOT "/usr/sbin/apk.static", O_WRONLY | O_CREAT | O_TRUNC, 0755);
            if (dst >= 0) {
                char cpbuf[4096]; ssize_t n;
                while ((n = read(src, cpbuf, sizeof(cpbuf))) > 0)
                    write(dst, cpbuf, n);
                close(dst);
            }
            close(src);
        }
    }

    // apk update (use full path to avoid PATH issues in chroot)
    int rc = sh_exec(ROOT, "/sbin/apk update 2>&1 || /usr/sbin/apk.static update 2>&1",
                     g_buf, BUF_SZ, 60000);
    if (rc == 0)
        pass("p7_apk_update");
    else {
        snprintf(g_detail, sizeof(g_detail), "rc=%d out='%.60s'", rc, g_buf);
        fail("p7_apk_update", g_detail);
        // Can't continue without update
        skip("p7_apk_add_file", "apk update failed");
        skip("p7_installed_binary", "apk update failed");
        skip("p7_apk_info", "apk update failed");
        return;
    }

    // apk add: install a small package. Try 'less' (tiny, few deps).
    // Use full path to apk binaries to avoid PATH issues.
    rc = sh_exec(ROOT, "/sbin/apk add --no-cache less 2>&1 || /usr/sbin/apk.static add --no-cache less 2>&1",
                 g_buf, BUF_SZ, 60000);
    if (rc == 0)
        pass("p7_apk_add_pkg");
    else {
        // Package download can fail due to network — not a kernel bug
        snprintf(g_detail, sizeof(g_detail), "rc=%d out='%.60s'", rc, g_buf);
        fail("p7_apk_add_pkg", g_detail);
    }

    // Run the installed binary
    rc = sh_exec(ROOT, "less --version 2>&1", g_buf, BUF_SZ, 10000);
    if (rc == 0 && strstr(g_buf, "less"))
        pass("p7_installed_binary");
    else {
        if (file_exists(ROOT "/usr/bin/less"))
            pass("p7_installed_binary");
        else {
            snprintf(g_detail, sizeof(g_detail), "rc=%d out='%.60s'", rc, g_buf);
            fail("p7_installed_binary", g_detail);
        }
    }

    // apk info: list installed packages
    rc = sh_exec(ROOT, "/sbin/apk info 2>&1 | wc -l", g_buf, BUF_SZ, 10000);
    if (rc == 0 && atoi(g_buf) >= 5) // Alpine has base packages
        pass("p7_apk_info");
    else {
        snprintf(g_detail, sizeof(g_detail), "rc=%d count='%.20s'", rc, g_buf);
        fail("p7_apk_info", g_detail);
    }
}

// ─── Phase 8: Stress & Edge Cases ───────────────────────────────────────────

static void phase8_stress(void) {
    printf("\n=== PHASE 8: Stress & Edge Cases ===\n");

    // Rapid fork+exit (50 children)
    {
        int ok = 1;
        for (int i = 0; i < 50; i++) {
            pid_t pid = fork();
            if (pid == 0) _exit(0);
            if (pid < 0) { ok = 0; break; }
            int status;
            waitpid(pid, &status, 0);
            if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) ok = 0;
        }
        if (ok)
            pass("p8_rapid_fork_50");
        else
            fail("p8_rapid_fork_50", "some children failed");
    }

    // Concurrent pipe chains (5 parallel)
    {
        pid_t pids[5];
        for (int i = 0; i < 5; i++) {
            pids[i] = fork();
            if (pids[i] == 0) {
                int rc = sh_exec(ROOT,
                    "seq 1 100 | sort -n | tail -1",
                    g_buf, BUF_SZ, 10000);
                _exit(rc == 0 && atoi(g_buf) == 100 ? 0 : 1);
            }
        }
        int ok = 1;
        for (int i = 0; i < 5; i++) {
            int status;
            waitpid(pids[i], &status, 0);
            if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) ok = 0;
        }
        if (ok)
            pass("p8_concurrent_pipes");
        else
            fail("p8_concurrent_pipes", "some pipe chains failed");
    }

    // Many small files (100 create + stat + unlink)
    {
        mkdir(ROOT "/tmp/stress_files", 0755);
        int ok = 1;
        for (int i = 0; i < 100; i++) {
            char name[64];
            snprintf(name, sizeof(name), ROOT "/tmp/stress_files/f%03d", i);
            int fd = open(name, O_CREAT | O_WRONLY | O_TRUNC, 0644);
            if (fd < 0) { ok = 0; break; }
            write(fd, "x", 1);
            close(fd);
        }
        // Verify all exist
        if (ok) {
            DIR *d = opendir(ROOT "/tmp/stress_files");
            int count = 0;
            if (d) {
                struct dirent *ent;
                while ((ent = readdir(d)) != NULL)
                    if (ent->d_name[0] != '.') count++;
                closedir(d);
            }
            if (count != 100) ok = 0;
        }
        // Unlink all
        for (int i = 0; i < 100; i++) {
            char name[64];
            snprintf(name, sizeof(name), ROOT "/tmp/stress_files/f%03d", i);
            unlink(name);
        }
        rmdir(ROOT "/tmp/stress_files");
        if (ok)
            pass("p8_100_files_create_unlink");
        else
            fail("p8_100_files_create_unlink", "count mismatch or create failed");
    }

    // Pipe throughput: 1MB through a pipe
    {
        int pfd[2];
        pipe(pfd);
        pid_t pid = fork();
        if (pid == 0) {
            close(pfd[0]);
            char block[4096];
            memset(block, 'P', sizeof(block));
            for (int i = 0; i < 256; i++)
                write(pfd[1], block, sizeof(block));
            close(pfd[1]);
            _exit(0);
        }
        close(pfd[1]);
        int total = 0;
        char rbuf[4096];
        ssize_t n;
        while ((n = read(pfd[0], rbuf, sizeof(rbuf))) > 0)
            total += n;
        close(pfd[0]);
        waitpid(pid, NULL, 0);
        if (total == 1048576)
            pass("p8_pipe_1mb");
        else {
            snprintf(g_detail, sizeof(g_detail), "got %d bytes", total);
            fail("p8_pipe_1mb", g_detail);
        }
    }

    // Signal storm: send 100 SIGUSRs to child
    {
        int pfd[2];
        pipe(pfd);
        pid_t pid = fork();
        if (pid == 0) {
            close(pfd[0]);
            volatile int count = 0;
            struct sigaction sa = {0};
            sa.sa_handler = SIG_IGN; // Ignore SIGUSR1
            sigaction(SIGUSR1, &sa, NULL);
            // Tell parent we're ready
            write(pfd[1], "R", 1);
            close(pfd[1]);
            // Spin for a bit to receive signals
            usleep(500000);
            _exit(0);
        }
        close(pfd[1]);
        // Wait for child to be ready
        char rdy;
        read(pfd[0], &rdy, 1);
        close(pfd[0]);
        // Send 100 signals
        for (int i = 0; i < 100; i++)
            kill(pid, SIGUSR1);
        int status;
        waitpid(pid, &status, 0);
        if (WIFEXITED(status) && WEXITSTATUS(status) == 0)
            pass("p8_signal_storm_100");
        else {
            snprintf(g_detail, sizeof(g_detail), "status=0x%x", status);
            fail("p8_signal_storm_100", g_detail);
        }
    }

    // dd: write 4MB file and verify size
    {
        int rc = sh_exec(ROOT,
            "dd if=/dev/zero of=/tmp/mmap_test bs=4096 count=1024 2>/dev/null && "
            "wc -c < /tmp/mmap_test",
            g_buf, BUF_SZ, 10000);
        if (rc == 0 && atoi(g_buf) == 4194304)
            pass("p8_dd_4mb");
        else {
            snprintf(g_detail, sizeof(g_detail), "rc=%d out='%.60s'", rc, g_buf);
            fail("p8_dd_4mb", g_detail);
        }
    }
}

// ─── Main ────────────────────────────────────────────────────────────────────

int main(void) {
    printf("TEST_START smoke_alpine\n");
    printf("Kevlar Alpine Smoke Test — comprehensive drop-in validation\n");

    struct timespec start;
    clock_gettime(CLOCK_MONOTONIC, &start);

    // Wait for DHCP
    sleep(3);

    // Per-phase timing with hang detection.
    // If a phase takes longer than its budget, print a warning.
    struct timespec phase_start, phase_end;
    #define PHASE_BEGIN(name, budget_s) do { \
        printf(">>> %s (budget %ds)\n", name, budget_s); \
        fflush(stdout); \
        clock_gettime(CLOCK_MONOTONIC, &phase_start); \
    } while(0)
    #define PHASE_END(name, budget_s) do { \
        clock_gettime(CLOCK_MONOTONIC, &phase_end); \
        int elapsed_s = (int)(phase_end.tv_sec - phase_start.tv_sec); \
        if (elapsed_s > 3 * (budget_s)) \
            printf("WARNING: %s took %ds (budget %ds, 3x=%ds)\n", \
                   name, elapsed_s, budget_s, 3*(budget_s)); \
        else \
            printf("<<< %s done (%ds)\n", name, elapsed_s); \
        fflush(stdout); \
    } while(0)

    // Phase 1: Boot
    PHASE_BEGIN("Phase 1: Boot", 5);
    int booted = phase1_boot();
    PHASE_END("Phase 1: Boot", 5);
    if (!booted) {
        printf("FATAL: Phase 1 boot failed, cannot continue\n");
        printf("TEST_END %d/%d passed\n", g_pass, g_pass + g_fail);
        return 1;
    }

    // Phase 2: Filesystem
    PHASE_BEGIN("Phase 2: Filesystem", 10);
    phase2_filesystem();
    PHASE_END("Phase 2: Filesystem", 10);

    // Phase 3: Shell & Utilities
    PHASE_BEGIN("Phase 3: Shell", 10);
    phase3_shell();
    PHASE_END("Phase 3: Shell", 10);

    // Phase 4: Process Management
    PHASE_BEGIN("Phase 4: Processes", 15);
    phase4_processes();
    PHASE_END("Phase 4: Processes", 15);

    // Phase 5: System Info
    PHASE_BEGIN("Phase 5: SysInfo", 5);
    phase5_sysinfo();
    PHASE_END("Phase 5: SysInfo", 5);

    // Phase 6: Networking (check if DNS works for later phases)
    PHASE_BEGIN("Phase 6: Networking", 15);
    int fail_before_net = g_fail;
    phase6_network();
    int net_ok = (g_fail == fail_before_net); // no new failures in phase 6
    PHASE_END("Phase 6: Networking", 15);

    // Phase 7: Package Management
    PHASE_BEGIN("Phase 7: Packages", 60);
    phase7_packages(net_ok);
    PHASE_END("Phase 7: Packages", 60);

    // Phase 8: Stress
    PHASE_BEGIN("Phase 8: Stress", 30);
    phase8_stress();
    PHASE_END("Phase 8: Stress", 30);

    struct timespec end;
    clock_gettime(CLOCK_MONOTONIC, &end);
    double elapsed = (end.tv_sec - start.tv_sec) + (end.tv_nsec - start.tv_nsec) / 1e9;

    printf("\n════════════════════════════════════════════════════════\n");
    printf("SMOKE TEST COMPLETE: %d passed, %d failed, %d skipped (%.1fs)\n",
           g_pass, g_fail, g_skip, elapsed);
    printf("TEST_END %d/%d\n", g_pass, g_pass + g_fail);
    return g_fail > 0 ? 1 : 0;
}
