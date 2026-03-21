// Simulate BusyBox login's group/credential setup for root user.
// Reproduces "can't set groups: Socket not connected" error.
#define _GNU_SOURCE
#include <fcntl.h>
#include <grp.h>
#include <pwd.h>
#include <unistd.h>
#include <sys/types.h>
#include <string.h>
#include <stdio.h>
#include <errno.h>
#include <sys/mount.h>
#include <sys/stat.h>

static void msg(const char *s) { write(1, s, strlen(s)); }

int main(void) {
    msg("=== login flow test ===\n");

    // Set up Alpine chroot (same as boot-alpine)
    mkdir("/mnt", 0755);
    mount("tmpfs", "/mnt", "tmpfs", 0, NULL);
    mkdir("/mnt/root", 0755);
    mount("none", "/mnt/root", "ext4", 0, NULL);
    mkdir("/mnt/root/proc", 0755);
    mount("proc", "/mnt/root/proc", "proc", 0, NULL);
    mkdir("/mnt/root/dev", 0755);
    mount("devtmpfs", "/mnt/root/dev", "devtmpfs", 0, NULL);

    // pivot_root
    mkdir("/mnt/root/oldroot", 0755);
    syscall(155, "/mnt/root", "/mnt/root/oldroot");
    chdir("/");

    msg("test1: setgroups(0, NULL)\n");
    errno = 0;
    int r = setgroups(0, NULL);
    char buf[128];
    int n = snprintf(buf, sizeof(buf), "  result=%d errno=%d (%s)\n",
        r, errno, strerror(errno));
    write(1, buf, n);

    msg("test2: initgroups(\"root\", 0)\n");
    errno = 0;
    r = initgroups("root", 0);
    n = snprintf(buf, sizeof(buf), "  result=%d errno=%d (%s)\n",
        r, errno, strerror(errno));
    write(1, buf, n);

    msg("test3: getgrouplist\n");
    gid_t groups[32];
    int ngroups = 32;
    errno = 0;
    r = getgrouplist("root", 0, groups, &ngroups);
    n = snprintf(buf, sizeof(buf), "  result=%d ngroups=%d errno=%d (%s)\n",
        r, ngroups, errno, strerror(errno));
    write(1, buf, n);

    msg("test4: read /etc/group\n");
    int fd = open("/etc/group", 0);
    n = snprintf(buf, sizeof(buf), "  open=/etc/group fd=%d errno=%d\n",
        fd, errno);
    write(1, buf, n);
    if (fd >= 0) {
        char gbuf[256];
        int nr = read(fd, gbuf, sizeof(gbuf) - 1);
        if (nr > 0) {
            gbuf[nr > 80 ? 80 : nr] = 0;
            n = snprintf(buf, sizeof(buf), "  content: %s\n", gbuf);
            write(1, buf, n);
        }
        close(fd);
    }

    msg("=== done ===\n");
    return 0;
}
