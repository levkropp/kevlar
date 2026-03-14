/* Contract: /proc root directory enumerates PIDs and "self". */
#define _GNU_SOURCE
#include <dirent.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int main(void) {
    /* readdir("/proc") should contain "self" */
    DIR *d = opendir("/proc");
    if (!d) {
        printf("CONTRACT_FAIL opendir_proc\n");
        return 1;
    }

    int found_self = 0;
    int found_pid = 0;
    struct dirent *ent;
    while ((ent = readdir(d)) != NULL) {
        if (strcmp(ent->d_name, "self") == 0) {
            found_self = 1;
        }
        /* Check for at least one numeric PID entry */
        if (ent->d_name[0] >= '1' && ent->d_name[0] <= '9') {
            found_pid = 1;
        }
    }
    closedir(d);

    if (!found_self) {
        printf("CONTRACT_FAIL proc_readdir_self\n");
        return 1;
    }
    printf("proc_readdir_self: ok\n");

    if (!found_pid) {
        printf("CONTRACT_FAIL proc_readdir_pid\n");
        return 1;
    }
    printf("proc_readdir_pid: ok\n");

    /* /proc/self should resolve via readlink to something containing our PID */
    char buf[64];
    int len = readlink("/proc/self", buf, sizeof(buf) - 1);
    if (len <= 0) {
        printf("CONTRACT_FAIL proc_self_readlink\n");
        return 1;
    }
    buf[len] = '\0';
    int my_pid = getpid();
    char pid_str[32];
    snprintf(pid_str, sizeof(pid_str), "%d", my_pid);
    /* Accept either "PID" (Linux) or "/proc/PID" (Kevlar) */
    char *tail = strrchr(buf, '/');
    const char *pid_part = tail ? tail + 1 : buf;
    if (strcmp(pid_part, pid_str) != 0) {
        printf("CONTRACT_FAIL proc_self_target expected=%s got=%s\n", pid_str, buf);
        return 1;
    }
    printf("proc_self_readlink: ok\n");

    /* /proc/1/stat should be readable */
    int fd = open("/proc/1/stat", O_RDONLY);
    if (fd < 0) {
        printf("CONTRACT_FAIL proc_1_stat_open\n");
        return 1;
    }
    int nr = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (nr <= 0) {
        printf("CONTRACT_FAIL proc_1_stat_read\n");
        return 1;
    }
    printf("proc_1_stat: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
