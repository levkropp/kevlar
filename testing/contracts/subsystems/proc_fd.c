/* Contract: /proc/self/fd/ lists open descriptors and readlink resolves them. */
#define _GNU_SOURCE
#include <dirent.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int main(void) {
    /* stdin/stdout/stderr should be open */
    DIR *dir = opendir("/proc/self/fd");
    if (!dir) {
        printf("CONTRACT_FAIL proc_fd_opendir\n");
        return 1;
    }

    int found_0 = 0, found_1 = 0, found_2 = 0;
    int total = 0;
    struct dirent *de;
    while ((de = readdir(dir)) != NULL) {
        if (de->d_name[0] == '.') continue;
        int fd_num = atoi(de->d_name);
        if (fd_num == 0) found_0 = 1;
        if (fd_num == 1) found_1 = 1;
        if (fd_num == 2) found_2 = 1;
        total++;
    }
    closedir(dir);

    if (!found_0 || !found_1 || !found_2) {
        printf("CONTRACT_FAIL proc_fd_stdio: 0=%d 1=%d 2=%d\n",
               found_0, found_1, found_2);
        return 1;
    }
    printf("proc_fd_stdio: ok\n");

    /* readlink on fd 0 should return a path */
    char buf[256];
    int len = readlink("/proc/self/fd/0", buf, sizeof(buf) - 1);
    if (len <= 0) {
        printf("CONTRACT_FAIL proc_fd_readlink\n");
        return 1;
    }
    buf[len] = '\0';
    printf("proc_fd_readlink: ok\n");

    /* Open /dev/null and verify it appears */
    int fd = open("/dev/null", O_RDONLY);
    if (fd < 0) {
        printf("CONTRACT_FAIL proc_fd_open_null\n");
        return 1;
    }
    char link_path[64];
    snprintf(link_path, sizeof(link_path), "/proc/self/fd/%d", fd);
    len = readlink(link_path, buf, sizeof(buf) - 1);
    close(fd);
    if (len <= 0) {
        printf("CONTRACT_FAIL proc_fd_null_readlink\n");
        return 1;
    }
    buf[len] = '\0';
    /* On Linux this is /dev/null; on Kevlar it may be /dev/null too */
    if (strstr(buf, "null") == NULL) {
        printf("CONTRACT_FAIL proc_fd_null_path: got '%s'\n", buf);
        return 1;
    }
    printf("proc_fd_null: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
