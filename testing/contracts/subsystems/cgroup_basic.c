/* Contract: /proc/self/cgroup returns valid cgroup v2 format. */
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int main(void) {
    char buf[256];

    /* /proc/self/cgroup should exist and return "0::/<path>\n" format */
    int fd = open("/proc/self/cgroup", O_RDONLY);
    if (fd < 0) {
        printf("CONTRACT_FAIL cgroup_open\n");
        return 1;
    }
    int nr = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (nr <= 0) {
        printf("CONTRACT_FAIL cgroup_read\n");
        return 1;
    }
    buf[nr] = '\0';

    /* Must start with "0::" (cgroups v2 unified hierarchy) */
    if (strncmp(buf, "0::", 3) != 0) {
        printf("CONTRACT_FAIL cgroup_format: got '%s'\n", buf);
        return 1;
    }
    printf("cgroup_format: ok\n");

    /* Path after "0::" must start with "/" */
    if (buf[3] != '/') {
        printf("CONTRACT_FAIL cgroup_path: got '%s'\n", buf);
        return 1;
    }
    printf("cgroup_path: ok\n");

    /* /proc/filesystems should list cgroup2 */
    fd = open("/proc/filesystems", O_RDONLY);
    if (fd >= 0) {
        nr = read(fd, buf, sizeof(buf) - 1);
        close(fd);
        if (nr > 0) {
            buf[nr] = '\0';
            if (strstr(buf, "cgroup2")) {
                printf("cgroup_filesystems: ok\n");
            } else {
                printf("CONTRACT_FAIL cgroup_filesystems: cgroup2 not listed\n");
                return 1;
            }
        }
    }

    printf("CONTRACT_PASS\n");
    return 0;
}
