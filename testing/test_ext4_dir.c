#include <fcntl.h>
#include <unistd.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <string.h>
#include <stdio.h>
#include <dirent.h>

int main(void) {
    write(1, "=== ext4 dir test ===\n", 22);
    mkdir("/mnt", 0755);
    mount("tmpfs", "/mnt", "tmpfs", 0, NULL);
    mkdir("/mnt/r", 0755);
    mount("none", "/mnt/r", "ext4", 0, NULL);
    mkdir("/mnt/r/old", 0755);
    syscall(155, "/mnt/r", "/mnt/r/old");
    chdir("/");

    // Check directory sizes
    struct stat st;
    char buf[256];
    const char *dirs[] = { "/lib/apk/db", "/lib/apk", "/lib", "/usr/lib", "/etc/apk", NULL };
    for (int i = 0; dirs[i]; i++) {
        stat(dirs[i], &st);
        int n = snprintf(buf, sizeof(buf), "%s: size=%ld blocks=%ld\n",
            dirs[i], (long)st.st_size, (long)st.st_blocks);
        write(1, buf, n);

        // Try readdir
        DIR *d = opendir(dirs[i]);
        if (d) {
            int count = 0;
            struct dirent *ent;
            while ((ent = readdir(d)) != NULL) count++;
            closedir(d);
            n = snprintf(buf, sizeof(buf), "  entries: %d\n", count);
            write(1, buf, n);
        }
    }
    write(1, "=== done ===\n", 13);
    return 0;
}
