/* Contract: /proc/self/stat and /proc/self/status report real per-process values. */
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/types.h>

int main(void) {
    char buf[1024];

    /* ── /proc/self/stat ─────────────────────────────────────────── */
    int fd = open("/proc/self/stat", O_RDONLY);
    if (fd < 0) {
        printf("CONTRACT_FAIL proc_pid_stat_open\n");
        return 1;
    }
    int nr = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (nr <= 0) {
        printf("CONTRACT_FAIL proc_pid_stat_read\n");
        return 1;
    }
    buf[nr] = '\0';

    /* Parse pid (field 1) */
    int stat_pid = atoi(buf);
    if (stat_pid != getpid()) {
        printf("CONTRACT_FAIL proc_pid_stat_pid: got %d expected %d\n",
               stat_pid, getpid());
        return 1;
    }
    printf("proc_pid_stat_pid: ok\n");

    /* Parse state (field 3) — find closing ')' then state char */
    char *paren = strrchr(buf, ')');
    if (!paren || paren[1] != ' ') {
        printf("CONTRACT_FAIL proc_pid_stat_format\n");
        return 1;
    }
    char state = paren[2];
    if (state != 'R') {
        printf("CONTRACT_FAIL proc_pid_stat_state: expected 'R' got '%c'\n", state);
        return 1;
    }
    printf("proc_pid_stat_state: ok\n");

    /* Parse num_threads (field 20) — count space-separated fields after ')' */
    char *p = paren + 2; /* points to state char */
    /* Fields after state: ppid(4) pgrp(5) session(6) tty_nr(7) tpgid(8)
       flags(9) minflt(10) cminflt(11) majflt(12) cmajflt(13) utime(14)
       stime(15) cutime(16) cstime(17) priority(18) nice(19) num_threads(20) */
    int field = 3; /* state is field 3 */
    while (field < 20 && *p) {
        if (*p == ' ') {
            field++;
            while (*p == ' ') p++;
        } else {
            p++;
        }
    }
    int num_threads = atoi(p);
    if (num_threads < 1) {
        printf("CONTRACT_FAIL proc_pid_num_threads: %d\n", num_threads);
        return 1;
    }
    printf("proc_pid_num_threads: ok (%d)\n", num_threads);

    /* ── /proc/self/status ───────────────────────────────────────── */
    fd = open("/proc/self/status", O_RDONLY);
    if (fd < 0) {
        printf("CONTRACT_FAIL proc_pid_status_open\n");
        return 1;
    }
    nr = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (nr <= 0) {
        printf("CONTRACT_FAIL proc_pid_status_read\n");
        return 1;
    }
    buf[nr] = '\0';

    /* Verify Name field is present */
    if (!strstr(buf, "Name:")) {
        printf("CONTRACT_FAIL proc_pid_status_name\n");
        return 1;
    }
    printf("proc_pid_status_name: ok\n");

    /* Verify Pid field matches getpid() */
    char pid_needle[32];
    snprintf(pid_needle, sizeof(pid_needle), "Pid:\t%d\n", getpid());
    if (!strstr(buf, pid_needle)) {
        printf("CONTRACT_FAIL proc_pid_status_pid\n");
        return 1;
    }
    printf("proc_pid_status_pid: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
