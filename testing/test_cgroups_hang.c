// Minimal cgroups v2 reproducer: replicates what Alpine's /etc/init.d/cgroups
// and rc-cgroup.sh do, step by step, with printf after each operation.
// The first step that hangs or fails reveals the kernel bug.
//
// Build: musl-gcc -static -o test-cgroups-hang test_cgroups_hang.c
// Run:   as PID 1 init or from a boot shim after pivot_root into Alpine
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <signal.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

static void msg(const char *s) { write(1, s, strlen(s)); }

static int step_ok(int n, const char *desc) {
    char buf[128];
    int len = snprintf(buf, sizeof(buf), "STEP %d: %s ... OK\n", n, desc);
    write(1, buf, len);
    return 1;
}

static int step_fail(int n, const char *desc, int err) {
    char buf[128];
    int len = snprintf(buf, sizeof(buf), "STEP %d: %s ... FAIL (errno=%d)\n", n, desc, err);
    write(1, buf, len);
    return 0;
}

static int write_file(const char *path, const char *data) {
    int fd = open(path, O_WRONLY);
    if (fd < 0) return -1;
    int n = write(fd, data, strlen(data));
    close(fd);
    return n < 0 ? -1 : 0;
}

static int read_file(const char *path, char *buf, int bufsize) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) return -1;
    int n = read(fd, buf, bufsize - 1);
    close(fd);
    if (n < 0) return -1;
    buf[n] = '\0';
    return n;
}

int main(void) {
    msg("=== Cgroups v2 Hang Reproducer ===\n");
    char buf[512];
    int rc;

    // Step 1: Check /proc/filesystems for cgroup2
    msg("STEP 1: grep cgroup2 /proc/filesystems ...\n");
    rc = read_file("/proc/filesystems", buf, sizeof(buf));
    if (rc < 0) { step_fail(1, "read /proc/filesystems", errno); return 1; }
    if (!strstr(buf, "cgroup2")) { step_fail(1, "cgroup2 not found", 0); return 1; }
    step_ok(1, "cgroup2 found in /proc/filesystems");

    // Step 2: Mount cgroup2 (like cgroup2_base)
    // The kernel already mounts cgroupfs at /sys/fs/cgroup during init.
    // The Alpine script tries to mount again. Check if already mounted.
    msg("STEP 2: mount -t cgroup2 /sys/fs/cgroup ...\n");
    struct stat st;
    int already_mounted = (stat("/sys/fs/cgroup/cgroup.controllers", &st) == 0);
    if (already_mounted) {
        step_ok(2, "cgroup2 already mounted (skipped mount)");
    } else {
        mkdir("/sys/fs/cgroup", 0755);
        rc = mount("none", "/sys/fs/cgroup", "cgroup2", MS_NODEV|MS_NOEXEC|MS_NOSUID, NULL);
        if (rc != 0) { step_fail(2, "mount cgroup2", errno); return 1; }
        step_ok(2, "mount cgroup2 succeeded");
    }

    // Step 3: Read cgroup.controllers
    msg("STEP 3: read cgroup.controllers ...\n");
    rc = read_file("/sys/fs/cgroup/cgroup.controllers", buf, sizeof(buf));
    if (rc < 0) { step_fail(3, "read cgroup.controllers", errno); return 1; }
    buf[strcspn(buf, "\n")] = '\0';
    char msg3[128];
    snprintf(msg3, sizeof(msg3), "controllers='%s'", buf);
    step_ok(3, msg3);

    // Step 4: Enable controllers via subtree_control
    msg("STEP 4: write +cpu +memory +pids to subtree_control ...\n");
    const char *controllers[] = {"+cpu", "+memory", "+pids", NULL};
    for (int i = 0; controllers[i]; i++) {
        char desc[64];
        snprintf(desc, sizeof(desc), "echo '%s' > cgroup.subtree_control", controllers[i]);
        msg("  ");
        msg(desc);
        msg(" ...\n");
        rc = write_file("/sys/fs/cgroup/cgroup.subtree_control", controllers[i]);
        if (rc != 0) {
            step_fail(4, desc, errno);
            // Non-fatal — continue with other controllers
        }
    }
    step_ok(4, "subtree_control writes done");

    // Step 5: Create a child cgroup (like rc-cgroup.sh cgroup2_set_limits)
    msg("STEP 5: mkdir /sys/fs/cgroup/openrc.test ...\n");
    rc = mkdir("/sys/fs/cgroup/openrc.test", 0755);
    if (rc != 0 && errno != EEXIST) { step_fail(5, "mkdir", errno); return 1; }
    step_ok(5, "mkdir openrc.test");

    // Step 6: Write PID 0 (self) to cgroup.procs
    msg("STEP 6: echo 0 > openrc.test/cgroup.procs ...\n");
    rc = write_file("/sys/fs/cgroup/openrc.test/cgroup.procs", "0");
    if (rc != 0) {
        step_fail(6, "write cgroup.procs", errno);
        // Non-fatal — continue
    } else {
        step_ok(6, "moved self to openrc.test");
    }

    // Step 6b: Read /proc/self/mountinfo while in child cgroup
    msg("STEP 6b: opening /proc/self/mountinfo ...\n");
    {
        int mfd = open("/proc/self/mountinfo", O_RDONLY);
        if (mfd < 0) {
            step_fail(6, "open mountinfo", errno);
        } else {
            msg("STEP 6b: open OK fd=");
            char fdstr[16];
            snprintf(fdstr, sizeof(fdstr), "%d", mfd);
            msg(fdstr);
            msg(", reading ...\n");
            rc = read(mfd, buf, sizeof(buf) - 1);
            msg("STEP 6b: read returned\n");
            close(mfd);
            if (rc < 0) {
                step_fail(6, "read mountinfo", errno);
            } else {
                buf[rc] = '\0';
                step_ok(6, "mountinfo read OK");
            }
        }
    }
    rc = 0; // Reset for next step
    if (rc < 0) {
        step_fail(6, "read /proc/self/mountinfo", errno);
    } else {
        char msg6b[128];
        snprintf(msg6b, sizeof(msg6b), "mountinfo=%d bytes, has_cgroup=%d",
                 rc, strstr(buf, "cgroup") != NULL);
        step_ok(6, msg6b);
    }

    // Step 6c: fork+exec a STATIC binary while in child cgroup
    msg("STEP 6c: fork+exec /bin/busybox echo (static, in child cgroup) ...\n");
    {
        pid_t child = fork();
        if (child == 0) {
            char *argv[] = {"/bin/busybox", "echo", "CHILD_OK", NULL};
            execv("/bin/busybox", argv);
            _exit(127);
        }
        int wstatus;
        pid_t wpid = waitpid(child, &wstatus, 0);
        if (wpid < 0) {
            step_fail(6, "waitpid static", errno);
        } else {
            char msg6c[128];
            snprintf(msg6c, sizeof(msg6c), "static child exit=%d", WEXITSTATUS(wstatus));
            step_ok(6, msg6c);
        }
    }

    // Step 6d: fork a child that reads /proc/self/mountinfo (in child cgroup)
    msg("STEP 6d: fork child that reads /proc/self/mountinfo ...\n");
    {
        pid_t child = fork();
        if (child == 0) {
            // Child: try to read mountinfo
            char mbuf[512];
            int mfd = open("/proc/self/mountinfo", O_RDONLY);
            if (mfd < 0) { _exit(1); }
            int mn = read(mfd, mbuf, sizeof(mbuf) - 1);
            close(mfd);
            _exit(mn >= 0 ? 0 : 2);
        }
        alarm(5);
        int wstatus;
        pid_t wpid = waitpid(child, &wstatus, 0);
        alarm(0);
        if (wpid < 0) {
            step_fail(6, "waitpid fork-read", errno);
        } else if (WIFEXITED(wstatus) && WEXITSTATUS(wstatus) == 0) {
            step_ok(6, "child read mountinfo OK");
        } else {
            char msg6d[64];
            snprintf(msg6d, sizeof(msg6d), "child exit=%d sig=%d",
                     WIFEXITED(wstatus) ? WEXITSTATUS(wstatus) : -1,
                     WIFSIGNALED(wstatus) ? WTERMSIG(wstatus) : 0);
            step_fail(6, msg6d, 0);
        }
    }

    // Step 6e: fork+exec busybox cat /proc/self/mountinfo (in child cgroup)
    msg("STEP 6e: fork+exec busybox cat /proc/self/mountinfo ...\n");
    {
        pid_t child = fork();
        if (child == 0) {
            int devnull = open("/dev/null", O_WRONLY);
            if (devnull >= 0) { dup2(devnull, 1); close(devnull); }
            char *argv[] = {"/bin/busybox", "cat", "/proc/self/mountinfo", NULL};
            execv("/bin/busybox", argv);
            _exit(127);
        }
        alarm(5);
        int wstatus;
        pid_t wpid = waitpid(child, &wstatus, 0);
        alarm(0);
        if (wpid < 0) {
            step_fail(6, "waitpid exec-cat", errno);
        } else if (WIFEXITED(wstatus) && WEXITSTATUS(wstatus) == 0) {
            step_ok(6, "exec cat mountinfo OK");
        } else {
            char msg6e[64];
            snprintf(msg6e, sizeof(msg6e), "exec-cat exit=%d sig=%d",
                     WIFEXITED(wstatus) ? WEXITSTATUS(wstatus) : -1,
                     WIFSIGNALED(wstatus) ? WTERMSIG(wstatus) : 0);
            step_fail(6, msg6e, 0);
        }
    }

    // Step 7: Read cgroup.events
    msg("STEP 7: read openrc.test/cgroup.events ...\n");
    rc = read_file("/sys/fs/cgroup/openrc.test/cgroup.events", buf, sizeof(buf));
    if (rc < 0) {
        step_fail(7, "read cgroup.events", errno);
    } else {
        buf[strcspn(buf, "\n")] = '\0';
        char msg7[128];
        snprintf(msg7, sizeof(msg7), "events='%s'", buf);
        step_ok(7, msg7);
    }

    // Step 8: Move self back to root cgroup and rmdir
    msg("STEP 8: cleanup ...\n");
    write_file("/sys/fs/cgroup/cgroup.procs", "0");
    rc = rmdir("/sys/fs/cgroup/openrc.test");
    if (rc != 0) {
        step_fail(8, "rmdir openrc.test", errno);
    } else {
        step_ok(8, "rmdir openrc.test");
    }

    msg("=== All steps completed ===\n");
    return 0;
}
