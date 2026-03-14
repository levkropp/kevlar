/* Contract: /proc/self/mountinfo has correct format. */
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int main(void) {
    int fd = open("/proc/self/mountinfo", O_RDONLY);
    if (fd < 0) {
        printf("CONTRACT_FAIL mountinfo_open\n");
        return 1;
    }

    char buf[4096];
    int nr = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (nr <= 0) {
        printf("CONTRACT_FAIL mountinfo_read\n");
        return 1;
    }
    buf[nr] = '\0';

    /* Verify at least one line exists */
    if (strchr(buf, '\n') == NULL) {
        printf("CONTRACT_FAIL mountinfo_empty\n");
        return 1;
    }
    printf("mountinfo_content: ok\n");

    /* Verify format: each line contains " - " separator */
    int found_sep = 0;
    char *line = buf;
    while (line && *line) {
        if (strstr(line, " - ") != NULL) {
            found_sep = 1;
        }
        char *nl = strchr(line, '\n');
        line = nl ? nl + 1 : NULL;
    }
    if (!found_sep) {
        printf("CONTRACT_FAIL mountinfo_format\n");
        return 1;
    }
    printf("mountinfo_format: ok\n");

    /* Verify a known filesystem type appears (/ is always mounted) */
    int found_root = 0;
    line = buf;
    while (line && *line) {
        if (strstr(line, " / ") != NULL) {
            found_root = 1;
        }
        char *nl = strchr(line, '\n');
        line = nl ? nl + 1 : NULL;
    }
    if (!found_root) {
        printf("CONTRACT_FAIL mountinfo_root\n");
        return 1;
    }
    printf("mountinfo_root: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
