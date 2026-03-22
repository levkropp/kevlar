#define _GNU_SOURCE
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/sysmacros.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (getpid() == 1) {
        mkdir("/proc", 0755);
        mkdir("/sys", 0755);
        mkdir("/dev", 0755);
        mkdir("/tmp", 0755);
        mount("proc", "/proc", "proc", 0, NULL);
        mount("sysfs", "/sys", "sysfs", 0, NULL);
        mount("devtmpfs", "/dev", "devtmpfs", 0, NULL);
        mount("tmpfs", "/tmp", "tmpfs", 0, NULL);
        if (open("/dev/null", O_RDONLY) < 0)
            mknod("/dev/null", S_IFCHR | 0666, makedev(1, 3));
    }
    write(1, "MINIMAL_OK\n", 11);
    return 0;
}
