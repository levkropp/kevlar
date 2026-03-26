// Boot shim for OpenSSL test: mount ext4, pivot_root into Alpine,
// set up networking, then exec the test-openssl binary.
#define _GNU_SOURCE
#include <fcntl.h>
#include <unistd.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <sys/reboot.h>
#include <string.h>
#include <stdio.h>

static void msg(const char *s) { write(1, s, strlen(s)); }

int main(void) {
    msg("openssl-boot: starting\n");

    // Mount tmpfs + ext4
    mkdir("/mnt", 0755);
    mount("tmpfs", "/mnt", "tmpfs", 0, NULL);
    mkdir("/mnt/root", 0755);

    if (mount("none", "/mnt/root", "ext4", 0, NULL) != 0) {
        msg("FATAL: mount ext4 failed\n");
        reboot(0x4321fedc);
        return 1;
    }

    // Mount essential filesystems
    mkdir("/mnt/root/proc", 0755);
    mount("proc", "/mnt/root/proc", "proc", 0, NULL);
    mkdir("/mnt/root/sys", 0755);
    mount("sysfs", "/mnt/root/sys", "sysfs", 0, NULL);
    mkdir("/mnt/root/dev", 0755);
    mount("devtmpfs", "/mnt/root/dev", "devtmpfs", 0, NULL);
    mkdir("/mnt/root/run", 0755);
    mount("tmpfs", "/mnt/root/run", "tmpfs", 0, NULL);
    mkdir("/mnt/root/tmp", 01777);
    mount("tmpfs", "/mnt/root/tmp", "tmpfs", 0, NULL);

    // pivot_root
    mkdir("/mnt/root/oldroot", 0755);
    if (syscall(155, "/mnt/root", "/mnt/root/oldroot") != 0) {
        msg("FATAL: pivot_root failed\n");
        reboot(0x4321fedc);
        return 1;
    }
    chdir("/");
    umount2("/oldroot", MNT_DETACH);

    // Set up networking
    FILE *f = fopen("/tmp/net-setup.sh", "w");
    if (f) {
        fprintf(f,
            "#!/bin/sh\n"
            "ip link set lo up\n"
            "ip link set eth0 up\n"
            "ip addr add 10.0.2.15/24 dev eth0\n"
            "ip route add default via 10.0.2.2\n"
        );
        fclose(f);
        chmod("/tmp/net-setup.sh", 0755);
        char *sh_argv[] = {"/bin/sh", "/tmp/net-setup.sh", NULL};
        if (fork() == 0) {
            execv("/bin/sh", sh_argv);
            _exit(1);
        }
        int status;
        wait(&status);
    }

    char *test_envp[] = {
        "HOME=/root",
        "PATH=/usr/sbin:/usr/bin:/sbin:/bin",
        "TERM=vt100",
        NULL,
    };

    // Run cgroups hang reproducer (if present)
    if (access("/usr/bin/test-cgroups-hang", X_OK) == 0) {
        char *cg_argv[] = {"/usr/bin/test-cgroups-hang", NULL};
        pid_t cpid = fork();
        if (cpid == 0) {
            execve("/usr/bin/test-cgroups-hang", cg_argv, test_envp);
            _exit(127);
        }
        int cstatus;
        waitpid(cpid, &cstatus, 0);
    }

    // Run test-openssl
    if (access("/usr/bin/test-openssl", X_OK) != 0) {
        msg("TEST_FAIL test-openssl binary not found\n");
        msg("TEST_END 0/1\n");
        reboot(0x4321fedc);
        return 1;
    }

    char *test_argv[] = {"/usr/bin/test-openssl", NULL};

    pid_t pid = fork();
    if (pid == 0) {
        execve("/usr/bin/test-openssl", test_argv, test_envp);
        _exit(127);
    }
    int status;
    waitpid(pid, &status, 0);

    reboot(0x4321fedc);
    return 0;
}
