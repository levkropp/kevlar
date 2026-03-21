// Boot shim: mount ext4 root on /dev/vda, chroot into it, exec /sbin/init.
// This runs as PID 1 from the initramfs, then hands off to Alpine's init.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <string.h>
#include <stdio.h>

static void msg(const char *s) {
    write(1, s, strlen(s));
}

int main(void) {
    msg("kevlar: Alpine boot shim starting\n");

    // Mount a tmpfs at /mnt so we have a writable directory for the mount point.
    // The initramfs root is read-only (EROFS), so we can't mkdir on it.
    mkdir("/mnt", 0755);  // may already exist in initramfs
    mount("tmpfs", "/mnt", "tmpfs", 0, NULL);
    mkdir("/mnt/root", 0755);

    // Mount the ext4 rootfs (source path is ignored for ext2/ext4,
    // kernel uses the global block device directly)
    int r = mount("none", "/mnt/root", "ext4", 0, NULL);
    if (r != 0) {
        char buf[128];
        int n = snprintf(buf, sizeof(buf), "kevlar: mount ext4 failed (r=%d errno=%d)\n", r, errno);
        write(1, buf, n);
        msg("kevlar: FATAL — failed to mount ext4 on /mnt/root\n");
        // Try to drop to a shell for debugging
        char *sh_argv[] = { "/bin/sh", NULL };
        execv("/bin/sh", sh_argv);
        return 1;
    }
    msg("kevlar: ext4 rootfs mounted on /mnt/root\n");

    // Mount essential filesystems inside the new root BEFORE chroot.
    // BusyBox init's sysinit mount commands may fail inside chroot
    // if the kernel's mount resolution doesn't work across chroot boundary.
    mkdir("/mnt/root/proc", 0755);
    mount("proc", "/mnt/root/proc", "proc", 0, NULL);
    mkdir("/mnt/root/sys", 0755);
    mount("sysfs", "/mnt/root/sys", "sysfs", 0, NULL);
    mkdir("/mnt/root/dev", 0755);
    mount("devtmpfs", "/mnt/root/dev", "devtmpfs", 0, NULL);
    mkdir("/mnt/root/dev/pts", 0755);
    mkdir("/mnt/root/dev/shm", 0755);
    mkdir("/mnt/root/run", 0755);
    mount("tmpfs", "/mnt/root/run", "tmpfs", 0, NULL);
    mkdir("/mnt/root/tmp", 01777);
    mount("tmpfs", "/mnt/root/tmp", "tmpfs", 0, NULL);

    // Try pivot_root first (proper root switch, no path prefix issues).
    // Fallback to chroot if pivot_root isn't available.
    mkdir("/mnt/root/oldroot", 0755);
    r = syscall(155, "/mnt/root", "/mnt/root/oldroot"); // pivot_root
    if (r == 0) {
        msg("kevlar: pivot_root succeeded\n");
        chdir("/");
        // Unmount old root (best-effort)
        umount2("/oldroot", MNT_DETACH);
    } else {
        msg("kevlar: pivot_root failed, falling back to chroot\n");
        r = chroot("/mnt/root");
        if (r != 0) {
            char buf[64];
            int n = snprintf(buf, sizeof(buf), "kevlar: chroot failed (errno=%d)\n", errno);
            write(1, buf, n);
            char *sh_argv[] = { "/bin/sh", NULL };
            execv("/bin/sh", sh_argv);
            return 1;
        }
        chdir("/");
    }

    msg("kevlar: exec /sbin/init\n");

    // Execute Alpine's init (BusyBox init reads /etc/inittab)
    char *init_argv[] = { "/sbin/init", NULL };
    char *init_envp[] = {
        "HOME=/root",
        "PATH=/usr/sbin:/usr/bin:/sbin:/bin",
        "TERM=vt100",
        NULL,
    };
    execve("/sbin/init", init_argv, init_envp);

    // If execve fails, drop to shell
    msg("kevlar: execve /sbin/init failed, dropping to shell\n");
    char *sh_argv[] = { "/bin/sh", NULL };
    char *sh_envp[] = {
        "HOME=/root",
        "PATH=/usr/sbin:/usr/bin:/sbin:/bin",
        "TERM=vt100",
        NULL,
    };
    execve("/bin/sh", sh_argv, sh_envp);
    return 1;
}
