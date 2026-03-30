// Diagnose ext4 multi-block directory readdir bug.
// Installs python3 (creates 200+ entry dir), then reads raw directory
// data via read() on the directory fd and dumps hex at block boundaries.
#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

static void msg(const char *s) { write(1, s, strlen(s)); }

static int run(const char *path, char *const argv[]) {
    pid_t pid = fork();
    if (pid == 0) { execv(path, argv); _exit(127); }
    int status; waitpid(pid, &status, 0);
    return WIFEXITED(status) ? WEXITSTATUS(status) : -1;
}

static void hexdump(const char *label, const unsigned char *data, int len) {
    char buf[256];
    int n = snprintf(buf, sizeof(buf), "%s: ", label);
    for (int i = 0; i < len && n < 240; i++) {
        n += snprintf(buf + n, sizeof(buf) - n, "%02x ", data[i]);
    }
    buf[n++] = '\n';
    write(1, buf, n);
}

static void setup_alpine_root(void) {
    mkdir("/mnt", 0755);
    mount("tmpfs", "/mnt", "tmpfs", 0, NULL);
    mkdir("/mnt/root", 0755);
    mount("none", "/mnt/root", "ext4", 0, NULL);
    mkdir("/mnt/root/proc", 0755);
    mount("proc", "/mnt/root/proc", "proc", 0, NULL);
    mkdir("/mnt/root/dev", 0755);
    mount("devtmpfs", "/mnt/root/dev", "devtmpfs", 0, NULL);
    mkdir("/mnt/root/tmp", 01777);
    mount("tmpfs", "/mnt/root/tmp", "tmpfs", 0, NULL);
    mkdir("/mnt/root/run", 0755);
    mount("tmpfs", "/mnt/root/run", "tmpfs", 0, NULL);
    mkdir("/mnt/root/oldroot", 0755);
    syscall(155, "/mnt/root", "/mnt/root/oldroot");
    chdir("/");
    system("/sbin/ip link set lo up");
    system("/sbin/ip link set eth0 up");
    system("/sbin/ip addr add 10.0.2.15/24 dev eth0");
    system("/sbin/ip route add default via 10.0.2.2");
}

// Walk ext4 directory entries from raw data, report per-block stats
static void walk_raw_dir(const char *path) {
    struct stat st;
    if (stat(path, &st) != 0) {
        printf("stat(%s) failed: %s\n", path, strerror(errno));
        return;
    }
    printf("\nDirectory: %s (size=%ld, %ld blocks)\n", path,
           (long)st.st_size, (long)(st.st_size / 4096));

    // Count entries via readdir
    DIR *d = opendir(path);
    if (!d) {
        printf("opendir failed: %s\n", strerror(errno));
        return;
    }
    int total = 0;
    int found_collections = 0;
    struct dirent *ent;
    while ((ent = readdir(d))) {
        total++;
        if (strcmp(ent->d_name, "collections") == 0) found_collections = 1;
    }
    closedir(d);
    printf("readdir: %d entries, collections=%s\n", total,
           found_collections ? "YES" : "NO");

    // Now use getdents64 syscall directly with a large buffer
    int fd = open(path, O_RDONLY | O_DIRECTORY);
    if (fd < 0) {
        printf("open O_DIRECTORY failed: %s\n", strerror(errno));
        return;
    }

    char buf[65536];
    int total_gd = 0;
    int found_gd = 0;
    long total_bytes = 0;

    while (1) {
        long n = syscall(217, fd, buf, sizeof(buf)); // SYS_getdents64
        if (n <= 0) break;
        total_bytes += n;
        long pos = 0;
        while (pos < n) {
            unsigned long long d_ino;
            unsigned short d_reclen;
            unsigned char d_type;
            memcpy(&d_ino, buf + pos, 8);
            memcpy(&d_reclen, buf + pos + 16, 2);
            d_type = buf[pos + 18];
            char *d_name = buf + pos + 19;
            total_gd++;
            if (strcmp(d_name, "collections") == 0) {
                found_gd = 1;
                printf("getdents64: FOUND collections (ino=%llu type=%d)\n",
                       d_ino, d_type);
            }
            pos += d_reclen;
        }
    }
    close(fd);
    printf("getdents64: %d entries (%ld bytes), collections=%s\n",
           total_gd, total_bytes, found_gd ? "YES" : "NO");

    // Direct stat check
    char subpath[512];
    snprintf(subpath, sizeof(subpath), "%s/collections", path);
    if (stat(subpath, &st) == 0) {
        printf("stat(collections): mode=0%o size=%ld\n",
               st.st_mode, (long)st.st_size);
    } else {
        printf("stat(collections): FAILED (%s)\n", strerror(errno));
    }

    // Try to look up entries that should be in blocks 2/3
    // by doing stat on names alphabetically near 'collections'
    const char *near[] = {"codecs.py", "codeop.py", "collections",
                          "colorsys.py", "compileall.py", "concurrent",
                          "configparser.py", NULL};
    printf("Direct stat near collections:\n");
    for (int i = 0; near[i]; i++) {
        snprintf(subpath, sizeof(subpath), "%s/%s", path, near[i]);
        int ok = stat(subpath, &st) == 0;
        printf("  %s: %s\n", near[i], ok ? "EXISTS" : "NOT FOUND");
    }
}

int main(void) {
    setup_alpine_root();
    sleep(1);

    msg("=== readdir Debug Test ===\n");

    // First test: create a large directory manually on tmpfs
    msg("\n--- Test 1: Large tmpfs directory ---\n");
    mkdir("/tmp/bigdir", 0755);
    for (int i = 0; i < 300; i++) {
        char name[32];
        snprintf(name, sizeof(name), "/tmp/bigdir/file_%03d", i);
        int fd = open(name, O_WRONLY | O_CREAT, 0644);
        if (fd >= 0) close(fd);
    }
    {
        DIR *d = opendir("/tmp/bigdir");
        int count = 0;
        if (d) {
            while (readdir(d)) count++;
            closedir(d);
        }
        printf("tmpfs bigdir: %d entries (expected 302 = 300 + . + ..)\n", count);
    }

    // Install python3 to create the problematic ext4 directory
    msg("\n--- Test 2: Install python3 ---\n");
    {
        char *argv[] = {"/sbin/apk", "add", "--no-check-certificate", "-q", "python3", NULL};
        run("/sbin/apk", argv);
    }

    walk_raw_dir("/usr/lib/python3.12");

    msg("\n=== Done ===\n");
    return 0;
}
