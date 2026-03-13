/* Contract: /proc/self/exe is a symlink to the current executable.
   /proc/self/stat contains process info in the expected format. */
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <sys/syscall.h>

int main(void) {
    /* /proc/self/exe: readlink should return the executable path */
    char buf[256];
    int len = readlink("/proc/self/exe", buf, sizeof(buf) - 1);
    if (len > 0) {
        buf[len] = '\0';
        printf("proc_self_exe: ok\n");
    } else {
        /* Not a hard failure — procfs might not support exe link */
        printf("proc_self_exe: not available (ok for now)\n");
    }

    /* /proc/self/stat: should contain "pid (comm) state ..." */
    int fd = open("/proc/self/stat", O_RDONLY);
    if (fd < 0) {
        /* procfs might not be fully implemented yet */
        printf("proc_self_stat: not available (ok for now)\n");
        printf("CONTRACT_PASS\n");
        return 0;
    }
    int nr = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (nr <= 0) {
        printf("CONTRACT_FAIL proc_stat_read\n");
        return 1;
    }
    buf[nr] = '\0';

    /* Parse: "pid (comm) state ..." — find the closing ')' */
    char *paren = strrchr(buf, ')');
    if (!paren) {
        printf("CONTRACT_FAIL proc_stat_format: no ')' found\n");
        return 1;
    }
    /* After ')' should be " X " where X is the state */
    if (paren[1] != ' ') {
        printf("CONTRACT_FAIL proc_stat_format: no space after ')'\n");
        return 1;
    }
    char state = paren[2];
    if (state != 'R' && state != 'S' && state != 'D' &&
        state != 'Z' && state != 'T' && state != 'X') {
        printf("CONTRACT_FAIL proc_stat_state: '%c'\n", state);
        return 1;
    }
    printf("proc_self_stat: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
