// ext4 persistence across reboot: write a file, reboot, read it back.
// Two-phase test: auto-detects phase by checking if the token file exists.
// Phase 1: write token + sync + reboot. Phase 2: read token + verify.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/reboot.h>
#include <sys/stat.h>
#include <unistd.h>

static void msg(const char *s) { write(1, s, strlen(s)); }

static const char *TOKEN_PATH = "/mnt/root/var/persistence_token";
static const char *TOKEN_VALUE = "KEVLAR_PERSIST_OK_20260330";

int main(void) {
    msg("=== ext4 Persistence Test ===\n");

    // Mount ext4
    mkdir("/mnt", 0755);
    mount("tmpfs", "/mnt", "tmpfs", 0, NULL);
    mkdir("/mnt/root", 0755);
    if (mount("none", "/mnt/root", "ext4", 0, NULL) != 0) {
        msg("FATAL: mount ext4 failed\n");
        return 1;
    }
    mkdir("/mnt/root/var", 0755);

    // Check which phase we're in
    struct stat st;
    if (stat(TOKEN_PATH, &st) != 0) {
        // Phase 1: write token
        msg("Phase 1: writing token\n");
        int fd = open(TOKEN_PATH, O_WRONLY | O_CREAT | O_TRUNC, 0644);
        if (fd < 0) {
            char buf[128];
            int n = snprintf(buf, sizeof(buf), "FAIL: open write: %s\n", strerror(errno));
            write(1, buf, n);
            return 1;
        }
        write(fd, TOKEN_VALUE, strlen(TOKEN_VALUE));
        close(fd);
        sync();
        msg("Phase 1: token written, synced. Rebooting...\n");
        reboot(RB_AUTOBOOT);
        // Should not reach here
        msg("FAIL: reboot returned\n");
        return 1;
    } else {
        // Phase 2: read and verify
        msg("Phase 2: reading token\n");
        int fd = open(TOKEN_PATH, O_RDONLY);
        if (fd < 0) {
            char buf[128];
            int n = snprintf(buf, sizeof(buf), "FAIL: open read: %s\n", strerror(errno));
            write(1, buf, n);
            return 1;
        }
        char buf[128] = {0};
        int n = read(fd, buf, sizeof(buf) - 1);
        close(fd);

        if (n > 0 && strncmp(buf, TOKEN_VALUE, strlen(TOKEN_VALUE)) == 0) {
            msg("PASS: token matches after reboot\n");
            msg("TEST_PASS\n");
            // Clean up
            unlink(TOKEN_PATH);
            sync();
        } else {
            char errbuf[256];
            int en = snprintf(errbuf, sizeof(errbuf),
                "FAIL: token mismatch (got '%.*s', expected '%s')\n",
                n, buf, TOKEN_VALUE);
            write(1, errbuf, en);
            msg("TEST_FAIL\n");
        }
    }
    return 0;
}
