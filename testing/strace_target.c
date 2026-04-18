// PID-1 wrapper for strace-diff. Reads `strace-exec=path,arg1,arg2,...`
// from /proc/cmdline, mounts the virtio-blk rootfs at /mnt, chroots, and
// execs the command. The Kevlar kernel records every syscall this process
// makes (since we set strace-pid=1 on the cmdline).
//
// The cmdline separator is `,` instead of whitespace because `/proc/cmdline`
// values are already space-delimited.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <unistd.h>

#define ROOT "/mnt"
#define MAX_ARGS 32

static void die(const char *msg, int err) {
    fprintf(stderr, "strace-target: %s: errno=%d (%s)\n", msg, err, strerror(err));
    exit(1);
}

static char *find_in_cmdline(const char *needle, char *buf, size_t bufsz) {
    int fd = open("/proc/cmdline", O_RDONLY);
    if (fd < 0) return NULL;
    ssize_t n = read(fd, buf, bufsz - 1);
    close(fd);
    if (n <= 0) return NULL;
    buf[n] = 0;
    // Replace newline with space for uniform tokenization.
    for (ssize_t i = 0; i < n; i++) if (buf[i] == '\n') buf[i] = ' ';

    size_t nlen = strlen(needle);
    char *p = buf;
    while ((p = strstr(p, needle)) != NULL) {
        // Require start-of-string or space before.
        if (p != buf && p[-1] != ' ') { p++; continue; }
        if (p[nlen] != '=') { p++; continue; }
        return p + nlen + 1;  // value start
    }
    return NULL;
}

int main(void) {
    printf("strace-target: start\n"); fflush(stdout);

    // Read the command to exec from /proc/cmdline. Requires /proc.
    mkdir("/proc", 0755);
    mount("proc", "/proc", "proc", 0, NULL);

    static char cmdbuf[2048];
    char *val = find_in_cmdline("strace-exec", cmdbuf, sizeof(cmdbuf));
    if (!val || !*val) {
        fprintf(stderr, "strace-target: no strace-exec= on cmdline; running default /bin/true\n");
        val = "/bin/true";
    }
    // Clip at first space — next cmdline token.
    char *sp = strchr(val, ' ');
    if (sp) *sp = 0;
    // Duplicate because `val` points into static cmdbuf we overwrite below.
    static char cmd_copy[1024];
    strncpy(cmd_copy, val, sizeof(cmd_copy) - 1);
    cmd_copy[sizeof(cmd_copy) - 1] = 0;

    printf("strace-target: exec spec = \"%s\"\n", cmd_copy); fflush(stdout);

    // Mount the rootfs. Try ext4 then ext2, with and without partition table.
    mkdir(ROOT, 0755);
    int ok = 0;
    const char *devs[]  = { "/dev/vda1", "/dev/vda", NULL };
    const char *fss[]   = { "ext4", "ext2", NULL };
    for (int d = 0; devs[d] && !ok; d++) {
        for (int f = 0; fss[f] && !ok; f++) {
            if (mount(devs[d], ROOT, fss[f], MS_RELATIME, NULL) == 0) {
                printf("strace-target: mounted %s (%s)\n", devs[d], fss[f]);
                ok = 1;
            }
        }
    }
    if (!ok) die("mount rootfs", errno);
    mount("/dev", ROOT "/dev", NULL, MS_BIND, NULL);
    mount("/proc", ROOT "/proc", NULL, MS_BIND, NULL);
    mount("/sys", ROOT "/sys", NULL, MS_BIND, NULL);
    fflush(stdout);

    // Split cmd_copy on ',' into argv.
    char *argv[MAX_ARGS + 1];
    int argc = 0;
    char *tok = cmd_copy;
    while (tok && argc < MAX_ARGS) {
        char *comma = strchr(tok, ',');
        if (comma) { *comma = 0; }
        argv[argc++] = tok;
        tok = comma ? comma + 1 : NULL;
    }
    argv[argc] = NULL;

    printf("strace-target: chroot + exec %s", argv[0]);
    for (int i = 1; i < argc; i++) printf(" '%s'", argv[i]);
    printf("\n"); fflush(stdout);

    if (chroot(ROOT) != 0) die("chroot", errno);
    if (chdir("/") != 0) die("chdir", errno);

    char *envp[] = {
        "PATH=/usr/bin:/usr/sbin:/sbin:/bin",
        "HOME=/root", "TERM=vt100",
        "LANG=C", "LC_ALL=C",
        NULL
    };
    execve(argv[0], argv, envp);
    die("execve", errno);
    return 1;
}
