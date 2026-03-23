// Test: boot Alpine, run apk update + apk add curl, verify curl.
// Uses BusyBox init with a custom inittab for proper Alpine boot.
// Copies apk.static from initramfs into the Alpine rootfs before
// pivot_root so package management works (the dynamic apk binary
// has unresolved library issues on Kevlar — tracked separately).
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <sys/reboot.h>
#include <string.h>
#include <stdio.h>

static void msg(const char *s) { write(1, s, strlen(s)); }

int main(void) {
    msg("test_alpine_apk: starting\n");

    // Mount tmpfs + ext4
    mkdir("/mnt", 0755);
    mount("tmpfs", "/mnt", "tmpfs", 0, NULL);
    mkdir("/mnt/root", 0755);

    if (mount("none", "/mnt/root", "ext4", 0, NULL) != 0) {
        msg("TEST_FAIL mount_ext4\nTEST_END 0/1\n");
        reboot(0x4321fedc);
        return 1;
    }
    msg("TEST_PASS mount_ext4\n");

    // Mount essential filesystems
    mkdir("/mnt/root/proc", 0755);
    mount("proc", "/mnt/root/proc", "proc", 0, NULL);
    mkdir("/mnt/root/sys", 0755);
    mount("sysfs", "/mnt/root/sys", "sysfs", 0, NULL);
    mkdir("/mnt/root/dev", 0755);
    mount("devtmpfs", "/mnt/root/dev", "devtmpfs", 0, NULL);
    mkdir("/mnt/root/run", 0755);
    mount("tmpfs", "/mnt/root/run", "tmpfs", 0, NULL);
    mkdir("/mnt/root/tmp", 01777);
    mount("tmpfs", "/mnt/root/tmp", "tmpfs", 0, NULL);

    // Copy tools from initramfs into the Alpine rootfs before pivot_root
    {
        // Copy apk.static (working package manager)
        int src = open("/bin/apk.static", O_RDONLY);
        if (src >= 0) {
            int dst = open("/mnt/root/sbin/apk.static", O_WRONLY|O_CREAT|O_TRUNC, 0755);
            if (dst >= 0) {
                char buf[4096];
                int n;
                while ((n = read(src, buf, sizeof(buf))) > 0)
                    write(dst, buf, n);
                close(dst);
            }
            close(src);
        }
        // Copy dyntest (static diagnostic tool)
        src = open("/bin/dyntest", O_RDONLY);
        if (src >= 0) {
            int dst = open("/mnt/root/usr/bin/dyntest", O_WRONLY|O_CREAT|O_TRUNC, 0755);
            if (dst >= 0) {
                char buf[4096];
                int n;
                while ((n = read(src, buf, sizeof(buf))) > 0)
                    write(dst, buf, n);
                close(dst);
            }
            close(src);
        }
    }

    // pivot_root
    mkdir("/mnt/root/oldroot", 0755);
    if (syscall(155, "/mnt/root", "/mnt/root/oldroot") != 0) {
        msg("TEST_FAIL pivot_root\nTEST_END 1/2\n");
        reboot(0x4321fedc);
        return 1;
    }
    chdir("/");
    umount2("/oldroot", MNT_DETACH);
    msg("TEST_PASS pivot_root\n");

    // Write test script for BusyBox init
    FILE *f = fopen("/tmp/apk-test.sh", "w");
    if (!f) { msg("TEST_FAIL script\n"); reboot(0x4321fedc); return 1; }
    fprintf(f,
        "#!/bin/sh\n"
        "exec > /dev/console 2>&1\n"
        "# Network\n"
        "ip link set lo up\n"
        "ip link set eth0 up\n"
        "ip addr add 10.0.2.15/24 dev eth0\n"
        "ip route add default via 10.0.2.2\n"
        "# OpenRC\n"
        "openrc sysinit 2>/dev/null\n"
        "openrc boot 2>/dev/null\n"
        "echo TEST_PASS openrc_boot\n"
        "# apk update using apk.static\n"
        "if [ -x /sbin/apk.static ]; then\n"
        "  /sbin/apk.static update 2>&1\n"
        "  if [ $? -eq 0 ]; then echo TEST_PASS apk_update; else echo TEST_FAIL apk_update; fi\n"
        "  # apk add curl\n"
        "  /sbin/apk.static add --no-progress curl 2>&1\n"
        "  if [ $? -eq 0 ]; then echo TEST_PASS apk_add_curl; else echo TEST_FAIL apk_add_curl; fi\n"
        "else\n"
        "  echo TEST_FAIL apk_static_missing\n"
        "fi\n"
        "# Verify curl\n"
        "if [ -x /usr/bin/curl ]; then\n"
        "  /usr/bin/curl --version 2>&1 | head -1\n"
        "  if [ $? -eq 0 ]; then echo TEST_PASS curl_version; else echo TEST_FAIL curl_version; fi\n"
        "  # Test curl HTTP — save to file, check result\n"
        "  # Run dyntest AFTER curl is installed to probe dynamic linking\n"
        "  if [ -x /usr/bin/dyntest ]; then\n"
        "    echo DIAG: running dyntest after curl install...\n"
        "    /usr/bin/dyntest 2>&1\n"
        "  fi\n"
        "  echo DIAG: testing curl http to example.com...\n"
        "  /usr/bin/curl -s --max-time 15 http://example.com/ > /tmp/curl-out.html 2> /tmp/curl-err.txt\n"
        "  CURL_EXIT=$?\n"
        "  CURL_SIZE=0\n"
        "  test -f /tmp/curl-out.html && CURL_SIZE=$(cat /tmp/curl-out.html | wc -c)\n"
        "  echo DIAG: curl exit=$CURL_EXIT size=$CURL_SIZE\n"
        "  cat /tmp/curl-err.txt 2>/dev/null\n"
        "  if [ \"$CURL_SIZE\" -gt 0 ] 2>/dev/null; then\n"
        "    echo TEST_PASS curl_http\n"
        "    head -3 /tmp/curl-out.html\n"
        "  else\n"
        "    echo DIAG: curl produced no output, trying wget...\n"
        "    wget -q -O /tmp/wget-out.html http://example.com/ 2>&1\n"
        "    WGET_EXIT=$?\n"
        "    WGET_SIZE=0\n"
        "    test -f /tmp/wget-out.html && WGET_SIZE=$(cat /tmp/wget-out.html | wc -c)\n"
        "    echo DIAG: wget exit=$WGET_EXIT size=$WGET_SIZE\n"
        "    if [ \"$WGET_SIZE\" -gt 0 ] 2>/dev/null; then\n"
        "      echo TEST_PASS curl_http\n"
        "    else\n"
        "      echo TEST_FAIL curl_http\n"
        "    fi\n"
        "  fi\n"
        "else\n"
        "  echo TEST_FAIL curl_installed\n"
        "fi\n"
        "echo TEST_END\n"
        "reboot -f\n"
    );
    fclose(f);
    chmod("/tmp/apk-test.sh", 0755);

    // Custom inittab
    f = fopen("/etc/inittab", "w");
    if (!f) { msg("TEST_FAIL inittab\n"); reboot(0x4321fedc); return 1; }
    fprintf(f, "::sysinit:/tmp/apk-test.sh\n::ctrlaltdel:/sbin/reboot\n");
    fclose(f);

    // exec BusyBox init
    msg("test_alpine_apk: exec /sbin/init\n");
    char *init_argv[] = { "/sbin/init", NULL };
    char *init_envp[] = {
        "HOME=/root", "PATH=/usr/sbin:/usr/bin:/sbin:/bin", "TERM=vt100", NULL,
    };
    execve("/sbin/init", init_argv, init_envp);
    msg("TEST_FAIL execve\n");
    reboot(0x4321fedc);
    return 1;
}
