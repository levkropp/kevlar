/* Contract: /proc/self/maps shows VMA entries in the expected format. */
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/mman.h>

int main(void) {
    /* Map an anonymous page so we have a known mapping to find. */
    void *mapped = mmap(NULL, 4096, PROT_READ | PROT_WRITE,
                        MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (mapped == MAP_FAILED) {
        printf("CONTRACT_FAIL proc_maps_mmap\n");
        return 1;
    }

    /* Read /proc/self/maps */
    int fd = open("/proc/self/maps", O_RDONLY);
    if (fd < 0) {
        printf("CONTRACT_FAIL proc_maps_open\n");
        return 1;
    }
    char buf[4096];
    int nr = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (nr <= 0) {
        printf("CONTRACT_FAIL proc_maps_read\n");
        return 1;
    }
    buf[nr] = '\0';

    /* Verify [stack] annotation exists */
    if (!strstr(buf, "[stack]")) {
        printf("CONTRACT_FAIL proc_maps_stack\n");
        return 1;
    }
    printf("proc_maps_stack: ok\n");

    /* Verify [heap] annotation exists */
    if (!strstr(buf, "[heap]")) {
        printf("CONTRACT_FAIL proc_maps_heap\n");
        return 1;
    }
    printf("proc_maps_heap: ok\n");

    /* Verify format: at least one line matches "XXXXXXXX-XXXXXXXX rwxp" pattern */
    int found_perms = 0;
    char *line = buf;
    while (line && *line) {
        /* Each line should have a '-' separating start-end, then a space, then 4 perm chars */
        char *dash = strchr(line, '-');
        if (dash) {
            char *space = strchr(dash, ' ');
            if (space && space[1] && space[2] && space[3] && space[4]) {
                char p1 = space[1], p2 = space[2], p3 = space[3], p4 = space[4];
                if ((p1 == 'r' || p1 == '-') &&
                    (p2 == 'w' || p2 == '-') &&
                    (p3 == 'x' || p3 == '-') &&
                    (p4 == 'p' || p4 == 's')) {
                    found_perms = 1;
                }
            }
        }
        char *nl = strchr(line, '\n');
        line = nl ? nl + 1 : NULL;
    }
    if (!found_perms) {
        printf("CONTRACT_FAIL proc_maps_format\n");
        return 1;
    }
    printf("proc_maps_format: ok\n");

    /* Verify our mmap'd address appears in the output */
    char addr_str[32];
    snprintf(addr_str, sizeof(addr_str), "%08lx-", (unsigned long)mapped);
    if (!strstr(buf, addr_str)) {
        printf("CONTRACT_FAIL proc_maps_anon: expected %s\n", addr_str);
        return 1;
    }
    printf("proc_maps_anon: ok\n");

    munmap(mapped, 4096);

    printf("CONTRACT_PASS\n");
    return 0;
}
