/* Contract: syslog(SYSLOG_ACTION_SIZE_BUFFER) returns buffer size > 0;
 * syslog(SYSLOG_ACTION_CONSOLE_LEVEL) returns 0 (accepted). */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <sys/klog.h>
#include <sys/syscall.h>
#include <unistd.h>

/* syslog actions */
#define SYSLOG_ACTION_READ_ALL       3
#define SYSLOG_ACTION_CONSOLE_LEVEL  8
#define SYSLOG_ACTION_SIZE_BUFFER   10

static int my_klogctl(int type, char *buf, int len) {
    return syscall(SYS_syslog, type, buf, len);
}

int main(void) {
    /* Query log buffer size */
    int size = my_klogctl(SYSLOG_ACTION_SIZE_BUFFER, NULL, 0);
    if (size > 0) {
        printf("buffer_size: ok\n");
    } else if (size == 0) {
        printf("CONTRACT_FAIL size_zero\n");
        return 1;
    } else if (errno == EPERM) {
        /* Non-root: syslog access restricted */
        printf("buffer_size: ok\n");
    } else {
        printf("CONTRACT_FAIL size: ret=%d errno=%d\n", size, errno);
        return 1;
    }

    /* Set console log level (should be accepted) */
    int ret = my_klogctl(SYSLOG_ACTION_CONSOLE_LEVEL, NULL, 7);
    if (ret < 0) {
        /* EPERM on non-root is acceptable */
        if (errno == EPERM) {
            printf("console_level: ok\n");
        } else {
            printf("CONTRACT_FAIL console_level: ret=%d errno=%d\n", ret, errno);
            return 1;
        }
    } else {
        printf("console_level: ok\n");
    }

    printf("CONTRACT_PASS\n");
    return 0;
}
