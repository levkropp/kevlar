// Test: start Dropbear SSH server, verify keygen, startup, and listening.
// Runs directly in the initramfs (no Alpine ext4 rootfs needed).
//
// Note: guest-to-self TCP connections don't work in QEMU user-mode networking
// (SLIRP doesn't support hairpin NAT). Use `make run-alpine-ssh` with
// `ssh -p 2222 root@localhost` for end-to-end SSH testing from the host.
#define _GNU_SOURCE
#include <fcntl.h>
#include <unistd.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <sys/reboot.h>
#include <string.h>
#include <stdio.h>
#include <signal.h>

static void msg(const char *s) { write(1, s, strlen(s)); }

static int run_cmd(const char *path, char *const argv[]) {
    pid_t pid = fork();
    if (pid == 0) {
        execv(path, argv);
        _exit(127);
    }
    if (pid < 0) return -1;
    int status;
    waitpid(pid, &status, 0);
    if (WIFEXITED(status)) return WEXITSTATUS(status);
    if (WIFSIGNALED(status)) return 128 + WTERMSIG(status);
    return -1;
}

static int run_cmd_capture(const char *path, char *const argv[], char *out, int outsize) {
    int pfd[2];
    if (pipe(pfd) < 0) return -1;
    pid_t pid = fork();
    if (pid == 0) {
        close(pfd[0]);
        dup2(pfd[1], 1);
        dup2(pfd[1], 2);
        close(pfd[1]);
        execv(path, argv);
        _exit(127);
    }
    close(pfd[1]);
    int n = 0;
    while (n < outsize - 1) {
        int r = read(pfd[0], out + n, outsize - 1 - n);
        if (r <= 0) break;
        n += r;
    }
    out[n] = 0;
    close(pfd[0]);
    int status;
    waitpid(pid, &status, 0);
    if (WIFEXITED(status)) return WEXITSTATUS(status);
    if (WIFSIGNALED(status)) return 128 + WTERMSIG(status);
    return -1;
}

int main(void) {
    int pass = 0, total = 3;

    msg("test_ssh_dropbear: starting\n");

    // Mount essential filesystems
    mount("proc", "/proc", "proc", 0, NULL);
    mount("tmpfs", "/tmp", "tmpfs", 0, NULL);
    mount("sysfs", "/sys", "sysfs", 0, NULL);

    // Set up networking
    msg("DIAG: setting up network\n");
    {
        char *a1[] = {"/bin/busybox", "ip", "link", "set", "lo", "up", NULL};
        run_cmd("/bin/busybox", a1);
        char *a2[] = {"/bin/busybox", "ip", "link", "set", "eth0", "up", NULL};
        run_cmd("/bin/busybox", a2);
        char *a3[] = {"/bin/busybox", "ip", "addr", "add", "10.0.2.15/24", "dev", "eth0", NULL};
        run_cmd("/bin/busybox", a3);
        char *a4[] = {"/bin/busybox", "ip", "route", "add", "default", "via", "10.0.2.2", NULL};
        run_cmd("/bin/busybox", a4);
    }

    // Create necessary system files
    mkdir("/etc", 0755);
    mkdir("/etc/dropbear", 0755);
    mkdir("/root", 0700);
    {
        FILE *f = fopen("/etc/passwd", "w");
        if (f) { fprintf(f, "root:x:0:0:root:/root:/bin/sh\n"); fclose(f); }
        f = fopen("/etc/group", "w");
        if (f) { fprintf(f, "root:x:0:\n"); fclose(f); }
        f = fopen("/etc/shadow", "w");
        if (f) { fprintf(f, "root::0:0:99999:7:::\n"); fclose(f); }
        f = fopen("/etc/shells", "w");
        if (f) { fprintf(f, "/bin/sh\n"); fclose(f); }
    }

    // Test 1: Generate host key
    msg("DIAG: generating host key\n");
    {
        char output[2048] = {0};
        char *argv[] = {"/bin/dropbearkey", "-t", "ecdsa", "-f", "/tmp/dropbear_ecdsa_host_key", NULL};
        int ret = run_cmd_capture("/bin/dropbearkey", argv, output, sizeof(output));
        char buf[128];
        int n = snprintf(buf, sizeof(buf), "DIAG: dropbearkey exit=%d\n", ret);
        write(1, buf, n);
        if (ret == 0) {
            msg("TEST_PASS keygen\n");
            pass++;
        } else {
            msg("TEST_FAIL keygen\n");
            msg("DIAG: ");
            msg(output);
            msg("\n");
        }
    }

    // Test 2: Start dropbear
    msg("DIAG: starting dropbear on port 22\n");
    pid_t db_pid = fork();
    if (db_pid == 0) {
        char *argv[] = {"/bin/dropbear", "-r", "/tmp/dropbear_ecdsa_host_key", "-F", "-p", "22", NULL};
        execv("/bin/dropbear", argv);
        _exit(127);
    }
    sleep(3);

    if (kill(db_pid, 0) == 0) {
        msg("TEST_PASS dropbear_running\n");
        pass++;
    } else {
        msg("TEST_FAIL dropbear_running\n");
        int status;
        waitpid(db_pid, &status, WNOHANG);
        char buf[128];
        int n;
        if (WIFSIGNALED(status))
            n = snprintf(buf, sizeof(buf), "DIAG: dropbear killed by signal %d\n", WTERMSIG(status));
        else
            n = snprintf(buf, sizeof(buf), "DIAG: dropbear exited code=%d\n", WEXITSTATUS(status));
        write(1, buf, n);
    }

    // Test 3: Check listening sockets in /proc/net/tcp
    {
        char proc_tcp[4096] = {0};
        int fd = open("/proc/net/tcp", O_RDONLY);
        if (fd >= 0) {
            read(fd, proc_tcp, sizeof(proc_tcp) - 1);
            close(fd);
        }
        msg("DIAG: /proc/net/tcp:\n");
        msg(proc_tcp);
        // Check for LISTEN state (0A)
        if (strstr(proc_tcp, " 0A ")) {
            msg("TEST_PASS dropbear_listen\n");
            pass++;
        } else {
            msg("TEST_FAIL dropbear_listen\n");
        }
    }

    // Cleanup
    kill(db_pid, SIGTERM);
    waitpid(db_pid, NULL, 0);

    // Summary
    {
        char buf[64];
        int n = snprintf(buf, sizeof(buf), "TEST_END %d/%d\n", pass, total);
        write(1, buf, n);
    }
    if (pass == total)
        msg("ALL SSH TESTS PASSED\n");

    reboot(0x4321fedc);
    return 0;
}
