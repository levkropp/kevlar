// Multi-user test: adduser, chpasswd, su, permission enforcement.
// Runs on Alpine ext4 root after pivot_root.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

static int pass = 0, fail = 0;
static void msg(const char *s) { write(1, s, strlen(s)); }

static int run(const char *path, char *const argv[]) {
    pid_t pid = fork();
    if (pid == 0) { execv(path, argv); _exit(127); }
    int status; waitpid(pid, &status, 0);
    return WIFEXITED(status) ? WEXITSTATUS(status) : -1;
}

// Run a shell command
static int sh(const char *cmd) {
    char *argv[] = {"/bin/sh", "-c", (char *)cmd, NULL};
    return run("/bin/sh", argv);
}

static void setup_alpine_root(void) {
    mkdir("/mnt", 0755);
    mount("tmpfs", "/mnt", "tmpfs", 0, NULL);
    mkdir("/mnt/root", 0755);
    if (mount("none", "/mnt/root", "ext4", 0, NULL) != 0) {
        msg("FATAL: mount ext4 failed\n");
        _exit(1);
    }
    mkdir("/mnt/root/proc", 0755);
    mount("proc", "/mnt/root/proc", "proc", 0, NULL);
    mkdir("/mnt/root/dev", 0755);
    mount("devtmpfs", "/mnt/root/dev", "devtmpfs", 0, NULL);
    mkdir("/mnt/root/tmp", 01777);
    mount("tmpfs", "/mnt/root/tmp", "tmpfs", 0, NULL);
    mkdir("/mnt/root/oldroot", 0755);
    syscall(155, "/mnt/root", "/mnt/root/oldroot");
    chdir("/");
}

int main(void) {
    setup_alpine_root();

    msg("=== Multi-User Tests ===\n");

    // Test 1: adduser
    printf("TEST adduser -D testuser: ");
    {
        int r = sh("adduser -D testuser 2>/dev/null");
        if (r == 0) { printf("PASS\n"); pass++; }
        else { printf("FAIL (exit=%d)\n", r); fail++; }
    }

    // Test 2: verify /etc/passwd
    printf("TEST /etc/passwd has testuser: ");
    {
        int r = sh("grep -q '^testuser:' /etc/passwd");
        if (r == 0) { printf("PASS\n"); pass++; }
        else { printf("FAIL\n"); fail++; }
    }

    // Test 3: chpasswd
    printf("TEST chpasswd: ");
    {
        int r = sh("echo 'testuser:test123' | chpasswd 2>/dev/null");
        if (r == 0) { printf("PASS\n"); pass++; }
        else { printf("FAIL (exit=%d)\n", r); fail++; }
    }

    // Test 4: su to testuser
    printf("TEST su testuser -c whoami: ");
    {
        int r = sh("su testuser -c whoami 2>/dev/null");
        // su may not have setuid bit set, try running as root
        if (r == 0) { printf("PASS\n"); pass++; }
        else {
            // Try with busybox su directly
            r = sh("/bin/busybox su testuser -c whoami 2>/dev/null");
            if (r == 0) { printf("PASS (busybox su)\n"); pass++; }
            else { printf("FAIL (exit=%d)\n", r); fail++; }
        }
    }

    // Test 5: su -c id shows non-root
    printf("TEST su testuser -c id: ");
    {
        int r = sh("su testuser -c 'id -u' 2>/dev/null | grep -v '^0$' | grep -q '[0-9]'");
        if (r == 0) { printf("PASS\n"); pass++; }
        else { printf("FAIL\n"); fail++; }
    }

    // Test 6: File permission enforcement
    printf("TEST permission enforcement: ");
    {
        // Create root-only file
        int fd = open("/tmp/root_only", O_WRONLY | O_CREAT | O_TRUNC, 0600);
        if (fd >= 0) { write(fd, "secret\n", 7); close(fd); }
        // Try to read as testuser
        int r = sh("su testuser -c 'cat /tmp/root_only' 2>/dev/null");
        if (r != 0) {
            printf("PASS (access denied)\n"); pass++;
        } else {
            printf("FAIL (access allowed)\n"); fail++;
        }
        unlink("/tmp/root_only");
    }

    // Test 7: setuid/setgid enforcement
    printf("TEST setuid EPERM for non-root: ");
    {
        // Fork a child that drops to testuser uid, then tries setuid(0)
        pid_t pid = fork();
        if (pid == 0) {
            // Get testuser uid from /etc/passwd
            setuid(1000); // typical first user uid
            seteuid(1000);
            // Now try to become root
            if (setuid(0) == -1 && errno == EPERM) {
                _exit(0); // PASS
            }
            _exit(1); // FAIL
        }
        int status;
        waitpid(pid, &status, 0);
        if (WIFEXITED(status) && WEXITSTATUS(status) == 0) {
            printf("PASS\n"); pass++;
        } else {
            printf("FAIL\n"); fail++;
        }
    }

    printf("\n=== Results: %d PASS, %d FAIL ===\n", pass, fail);
    printf(fail == 0 ? "TEST_PASS\n" : "TEST_FAIL\n");
    return fail > 0 ? 1 : 0;
}
