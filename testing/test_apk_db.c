#include <fcntl.h>
#include <unistd.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <string.h>
#include <stdio.h>
#include <errno.h>
#include <sys/wait.h>
#include <dirent.h>

static void msg(const char *s) { write(1, s, strlen(s)); }

int main(void) {
    msg("=== apk db test ===\n");

    mkdir("/mnt", 0755);
    mount("tmpfs", "/mnt", "tmpfs", 0, NULL);
    mkdir("/mnt/root", 0755);
    mount("none", "/mnt/root", "ext4", 0, NULL);
    mkdir("/mnt/root/oldroot", 0755);
    syscall(155, "/mnt/root", "/mnt/root/oldroot");
    chdir("/");

    // Check key paths
    const char *paths[] = {
        "/lib/apk/db",
        "/lib/apk/db/lock",
        "/lib/apk/db/installed",
        "/etc/apk/repositories",
        "/var/cache/apk",
        NULL
    };
    char buf[256];
    for (int i = 0; paths[i]; i++) {
        struct stat st;
        int r = stat(paths[i], &st);
        int n = snprintf(buf, sizeof(buf), "stat %s: %s (mode=%o)\n",
            paths[i], r == 0 ? "OK" : strerror(errno), r == 0 ? st.st_mode : 0);
        write(1, buf, n);
    }

    // List /lib/apk/db/
    msg("ls /lib/apk/db/:\n");
    DIR *d = opendir("/lib/apk/db");
    if (d) {
        struct dirent *ent;
        while ((ent = readdir(d)) != NULL) {
            int n = snprintf(buf, sizeof(buf), "  %s\n", ent->d_name);
            write(1, buf, n);
        }
        closedir(d);
    } else {
        int n = snprintf(buf, sizeof(buf), "  opendir failed: %s\n", strerror(errno));
        write(1, buf, n);
    }

    // Try apk update
    msg("exec: apk update\n");
    int pid = fork();
    if (pid == 0) {
        char *argv[] = { "apk", "update", NULL };
        char *envp[] = { "PATH=/usr/sbin:/usr/bin:/sbin:/bin", NULL };
        execve("/sbin/apk", argv, envp);
        _exit(1);
    }
    int status;
    waitpid(pid, &status, 0);
    int n = snprintf(buf, sizeof(buf), "apk status: 0x%x\n", status);
    write(1, buf, n);

    msg("=== done ===\n");
    return 0;
}
