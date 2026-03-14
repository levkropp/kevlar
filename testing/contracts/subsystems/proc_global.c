/* Contract: global /proc files are readable and contain expected content. */
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static int read_file(const char *path, char *buf, int bufsz) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        printf("CONTRACT_FAIL open_%s\n", path);
        return -1;
    }
    int nr = read(fd, buf, bufsz - 1);
    close(fd);
    if (nr <= 0) {
        printf("CONTRACT_FAIL read_%s\n", path);
        return -1;
    }
    buf[nr] = '\0';
    return nr;
}

int main(void) {
    char buf[4096];

    /* /proc/cpuinfo: must contain "processor" and "cpu MHz" */
    if (read_file("/proc/cpuinfo", buf, sizeof(buf)) < 0) return 1;
    if (!strstr(buf, "processor")) {
        printf("CONTRACT_FAIL cpuinfo_processor\n");
        return 1;
    }
    if (!strstr(buf, "cpu")) {
        printf("CONTRACT_FAIL cpuinfo_cpu_field\n");
        return 1;
    }
    printf("proc_cpuinfo: ok\n");

    /* /proc/version: must contain "Kevlar" or "Linux" */
    if (read_file("/proc/version", buf, sizeof(buf)) < 0) return 1;
    if (!strstr(buf, "Kevlar") && !strstr(buf, "Linux")) {
        printf("CONTRACT_FAIL version_content\n");
        return 1;
    }
    printf("proc_version: ok\n");

    /* /proc/meminfo: must contain "MemTotal:" with value > 0 */
    if (read_file("/proc/meminfo", buf, sizeof(buf)) < 0) return 1;
    char *mt = strstr(buf, "MemTotal:");
    if (!mt) {
        printf("CONTRACT_FAIL meminfo_memtotal\n");
        return 1;
    }
    long mem_kb = strtol(mt + 9, NULL, 10);
    if (mem_kb <= 0) {
        printf("CONTRACT_FAIL meminfo_value got=%ld\n", mem_kb);
        return 1;
    }
    printf("proc_meminfo: ok\n");

    /* /proc/mounts: must have at least one line */
    if (read_file("/proc/mounts", buf, sizeof(buf)) < 0) return 1;
    if (strlen(buf) < 2) {
        printf("CONTRACT_FAIL mounts_empty\n");
        return 1;
    }
    printf("proc_mounts: ok\n");

    /* /proc/uptime: two floats > 0 */
    if (read_file("/proc/uptime", buf, sizeof(buf)) < 0) return 1;
    float up1 = 0, up2 = 0;
    if (sscanf(buf, "%f %f", &up1, &up2) != 2 || up1 <= 0) {
        printf("CONTRACT_FAIL uptime_parse\n");
        return 1;
    }
    printf("proc_uptime: ok\n");

    /* /proc/loadavg: must have 5 fields */
    if (read_file("/proc/loadavg", buf, sizeof(buf)) < 0) return 1;
    float la1, la2, la3;
    int running, total;
    if (sscanf(buf, "%f %f %f %d/%d", &la1, &la2, &la3, &running, &total) != 5) {
        printf("CONTRACT_FAIL loadavg_parse\n");
        return 1;
    }
    printf("proc_loadavg: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
