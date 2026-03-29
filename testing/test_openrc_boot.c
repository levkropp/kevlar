// Test: Install OpenRC on Alpine, then boot each service with timeouts.
// Identifies which services work and which hang on Kevlar.
//
// Build: musl-gcc -static -O2 -o test-openrc-boot test_openrc_boot.c
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
#include <signal.h>

static void msg(const char *s) { write(1, s, strlen(s)); }

static void copy_file(const char *src_path, const char *dst_path, int mode) {
    int src = open(src_path, O_RDONLY);
    if (src < 0) return;
    int dst = open(dst_path, O_WRONLY|O_CREAT|O_TRUNC, mode);
    if (dst >= 0) {
        char buf[4096]; int n;
        while ((n = read(src, buf, sizeof(buf))) > 0) write(dst, buf, n);
        close(dst);
    }
    close(src);
}

int main(void) {
    msg("=== OpenRC Boot Investigation ===\n");

    // Mount tmpfs + ext4
    mkdir("/mnt", 0755);
    mount("tmpfs", "/mnt", "tmpfs", 0, NULL);
    mkdir("/mnt/root", 0755);

    if (mount("none", "/mnt/root", "ext4", 0, NULL) != 0) {
        msg("FAIL: mount ext4\n");
        reboot(0x4321fedc);
        return 1;
    }
    msg("OK: mount ext4\n");

    // Mount essential filesystems
    mkdir("/mnt/root/proc", 0755);
    mount("proc", "/mnt/root/proc", "proc", 0, NULL);
    mkdir("/mnt/root/sys", 0755);
    mount("sysfs", "/mnt/root/sys", "sysfs", 0, NULL);
    mkdir("/mnt/root/dev", 0755);
    mount("devtmpfs", "/mnt/root/dev", "devtmpfs", 0, NULL);
    mkdir("/mnt/root/dev/pts", 0755);
    mount("devpts", "/mnt/root/dev/pts", "devpts", 0, NULL);
    mkdir("/mnt/root/dev/shm", 01777);
    mount("tmpfs", "/mnt/root/dev/shm", "tmpfs", 0, NULL);
    mkdir("/mnt/root/run", 0755);
    mount("tmpfs", "/mnt/root/run", "tmpfs", 0, NULL);
    mkdir("/mnt/root/tmp", 01777);
    mount("tmpfs", "/mnt/root/tmp", "tmpfs", 0, NULL);

    // Copy apk.static from initramfs
    copy_file("/bin/apk.static", "/mnt/root/sbin/apk.static", 0755);

    // pivot_root
    mkdir("/mnt/root/oldroot", 0755);
    if (syscall(155, "/mnt/root", "/mnt/root/oldroot") != 0) {
        msg("FAIL: pivot_root\n");
        reboot(0x4321fedc);
        return 1;
    }
    chdir("/");
    umount2("/oldroot", MNT_DETACH);
    msg("OK: pivot_root\n");

    // Write the investigation script
    FILE *f = fopen("/tmp/openrc-test.sh", "w");
    if (!f) { msg("FAIL: script create\n"); reboot(0x4321fedc); return 1; }
    fprintf(f,
        "#!/bin/sh\n"
        "exec > /dev/console 2>&1\n"
        "set -x\n"
        "\n"
        "# Manual network setup first (needed for apk)\n"
        "ip link set lo up\n"
        "ip link set eth0 up\n"
        "ip addr add 10.0.2.15/24 dev eth0\n"
        "ip route add default via 10.0.2.2\n"
        "echo 'nameserver 10.0.2.3' > /etc/resolv.conf\n"
        "echo DIAG: network_up\n"
        "\n"
        "# Install OpenRC\n"
        "echo DIAG: installing openrc...\n"
        "/sbin/apk.static update 2>&1 | tail -1\n"
        "/sbin/apk.static add --no-progress openrc 2>&1 | tail -3\n"
        "OPENRC_EXIT=$?\n"
        "echo DIAG: openrc install exit=$OPENRC_EXIT\n"
        "if [ $OPENRC_EXIT -ne 0 ]; then\n"
        "  echo TEST_FAIL openrc_install\n"
        "  reboot -f\n"
        "fi\n"
        "echo TEST_PASS openrc_install\n"
        "\n"
        "# Show what services exist in each runlevel\n"
        "echo DIAG: runlevels:\n"
        "for rl in sysinit boot default; do\n"
        "  if [ -d /etc/runlevels/$rl ]; then\n"
        "    svcs=$(ls /etc/runlevels/$rl/ 2>/dev/null | tr '\\n' ' ')\n"
        "    echo \"DIAG: $rl: $svcs\"\n"
        "  fi\n"
        "done\n"
        "\n"
        "# Show what init.d scripts exist\n"
        "echo DIAG: init.d scripts:\n"
        "ls /etc/init.d/ 2>&1 | tr '\\n' ' '\n"
        "echo\n"
        "\n"
        "# Mount cgroup2 for OpenRC\n"
        "mkdir -p /sys/fs/cgroup\n"
        "mount -t cgroup2 none /sys/fs/cgroup 2>/dev/null\n"
        "\n"
        "# Try running each sysinit service individually with timeout\n"
        "echo DIAG: === testing sysinit services ===\n"
        "for svc in /etc/runlevels/sysinit/*; do\n"
        "  svcname=$(basename $svc)\n"
        "  echo DIAG: starting $svcname...\n"
        "  timeout 10 /sbin/openrc-run $svc start 2>&1\n"
        "  rc=$?\n"
        "  if [ $rc -eq 0 ]; then\n"
        "    echo TEST_PASS svc_sysinit_$svcname\n"
        "  elif [ $rc -eq 124 ]; then\n"
        "    echo TEST_FAIL svc_sysinit_$svcname '(TIMEOUT)'\n"
        "  else\n"
        "    echo TEST_FAIL svc_sysinit_$svcname exit=$rc\n"
        "  fi\n"
        "done\n"
        "\n"
        "# Try running each boot service individually with timeout\n"
        "echo DIAG: === testing boot services ===\n"
        "for svc in /etc/runlevels/boot/*; do\n"
        "  svcname=$(basename $svc)\n"
        "  echo DIAG: starting $svcname...\n"
        "  timeout 10 /sbin/openrc-run $svc start 2>&1\n"
        "  rc=$?\n"
        "  if [ $rc -eq 0 ]; then\n"
        "    echo TEST_PASS svc_boot_$svcname\n"
        "  elif [ $rc -eq 124 ]; then\n"
        "    echo TEST_FAIL svc_boot_$svcname '(TIMEOUT)'\n"
        "  else\n"
        "    echo TEST_FAIL svc_boot_$svcname exit=$rc\n"
        "  fi\n"
        "done\n"
        "\n"
        "# Now try the real openrc command\n"
        "echo DIAG: === trying openrc sysinit ===\n"
        "timeout 30 /sbin/openrc sysinit 2>&1\n"
        "SYSINIT_RC=$?\n"
        "echo DIAG: openrc_sysinit exit=$SYSINIT_RC\n"
        "if [ $SYSINIT_RC -eq 0 ]; then\n"
        "  echo TEST_PASS openrc_sysinit\n"
        "else\n"
        "  echo TEST_FAIL openrc_sysinit exit=$SYSINIT_RC\n"
        "fi\n"
        "\n"
        "echo DIAG: === trying openrc boot ===\n"
        "timeout 30 /sbin/openrc boot 2>&1\n"
        "BOOT_RC=$?\n"
        "echo DIAG: openrc_boot exit=$BOOT_RC\n"
        "if [ $BOOT_RC -eq 0 ]; then\n"
        "  echo TEST_PASS openrc_boot\n"
        "else\n"
        "  echo TEST_FAIL openrc_boot exit=$BOOT_RC\n"
        "fi\n"
        "\n"
        "echo DIAG: === trying openrc default ===\n"
        "timeout 30 /sbin/openrc default 2>&1\n"
        "DEFAULT_RC=$?\n"
        "echo DIAG: openrc_default exit=$DEFAULT_RC\n"
        "if [ $DEFAULT_RC -eq 0 ]; then\n"
        "  echo TEST_PASS openrc_default\n"
        "else\n"
        "  echo TEST_FAIL openrc_default exit=$DEFAULT_RC\n"
        "fi\n"
        "\n"
        "# Phase 2: Test individual services via rc-service\n"
        "echo DIAG: === testing rc-service start ===\n"
        "# First run sysinit (empty) to initialize OpenRC state\n"
        "timeout 15 /sbin/openrc sysinit 2>&1 | tail -2\n"
        "\n"
        "# Test: Run openrc-run.sh directly (bypassing openrc-run C binary)\n"
        "# This tests if the shell script itself hangs\n"
        "# Instrument the openrc-run.sh shell script with tracing\n"
        "RCSH=$(find / -name 'openrc-run.sh' -path '*/rc/sh/*' 2>/dev/null | head -1)\n"
        "echo DIAG: openrc-run.sh at $RCSH\n"
        "if [ -n \"$RCSH\" ]; then\n"
        "  # Add tracing: insert 'set -x' after the shebang line\n"
        "  cp $RCSH ${RCSH}.orig\n"
        "  sed -i '2i\\exec 2>/tmp/openrc-trace.log' $RCSH\n"
        "  sed -i '3i\\set -x' $RCSH\n"
        "  echo DIAG: patched $RCSH with tracing\n"
        "  head -5 $RCSH\n"
        "fi\n"
        "\n"
        "# Also instrument rc-cgroup.sh\n"
        "CGSH=$(find / -name 'rc-cgroup.sh' 2>/dev/null | head -1)\n"
        "if [ -n \"$CGSH\" ]; then\n"
        "  sed -i '1a\\set -x' $CGSH\n"
        "  echo DIAG: patched $CGSH with tracing\n"
        "fi\n"
        "\n"
        "# Test while-read on different file types\n"
        "mkdir -p /sys/fs/cgroup/test.read 2>/dev/null\n"
        "\n"
        "echo DIAG: test1 while-read /etc/hostname...\n"
        "while read -r line; do echo \"DIAG: t1=$line\"; done < /etc/hostname\n"
        "echo DIAG: test1 exit=$?\n"
        "\n"
        "echo DIAG: test2 while-read /proc/version...\n"
        "while read -r line; do echo \"DIAG: t2=$line\"; done < /proc/version\n"
        "echo DIAG: test2 exit=$?\n"
        "\n"
        "echo DIAG: test3a cat-pipe-read...\n"
        "cat /sys/fs/cgroup/test.read/cgroup.events | while read -r key value; do echo \"DIAG: t3a k=$key v=$value\"; done\n"
        "echo DIAG: test3a exit=$?\n"
        "\n"
        "echo DIAG: test3b subshell-read root cgroup.procs...\n"
        "timeout 5 sh -c 'read -r line < /sys/fs/cgroup/cgroup.procs; echo line=$line' 2>&1\n"
        "echo DIAG: test3b exit=$?\n"
        "\n"
        "echo DIAG: test3c subshell-read procfs...\n"
        "timeout 5 sh -c 'read -r line < /proc/uptime; echo line=$line' 2>&1\n"
        "echo DIAG: test3c exit=$?\n"
        "\n"
        "echo DIAG: test3d subshell-read sysfs...\n"
        "timeout 5 sh -c 'read -r line < /sys/class/tty/ttyS0/dev; echo line=$line' 2>&1\n"
        "echo DIAG: test3d exit=$?\n"
        "\n"
        "rmdir /sys/fs/cgroup/test.read 2>/dev/null\n"
        "\n"
        "# Enable hostname service\n"
        "rc-update add hostname boot 2>&1\n"
        "\n"
        "# Reset OpenRC state and do a fresh boot with services\n"
        "rm -rf /run/openrc 2>/dev/null\n"
        "echo DIAG: running openrc sysinit+boot with hostname...\n"
        "timeout 30 /sbin/openrc sysinit 2>&1\n"
        "echo DIAG: sysinit exit=$?\n"
        "timeout 30 /sbin/openrc boot 2>&1\n"
        "HOST_RC=$?\n"
        "echo DIAG: boot exit=$HOST_RC\n"
        "echo DIAG: hostname=$(hostname)\n"
        "\n"
        "# Dump the trace log to see where the shell hung\n"
        "echo DIAG: === openrc-run.sh trace log ===\n"
        "if [ -f /tmp/openrc-trace.log ]; then\n"
        "  tail -40 /tmp/openrc-trace.log\n"
        "  echo DIAG: trace_lines=$(wc -l < /tmp/openrc-trace.log)\n"
        "else\n"
        "  echo DIAG: no trace log found\n"
        "fi\n"
        "echo DIAG: rc-service hostname exit=$HOST_RC\n"
        "if [ $HOST_RC -eq 0 ]; then\n"
        "  echo TEST_PASS svc_hostname\n"
        "else\n"
        "  echo TEST_FAIL svc_hostname exit=$HOST_RC\n"
        "fi\n"
        "\n"
        "# Test cgroups service (with timeout to avoid hang)\n"
        "echo DIAG: starting cgroups via rc-service...\n"
        "timeout 15 rc-service cgroups start 2>&1\n"
        "CG_RC=$?\n"
        "echo DIAG: rc-service cgroups exit=$CG_RC\n"
        "if [ $CG_RC -eq 0 ]; then\n"
        "  echo TEST_PASS svc_cgroups\n"
        "else\n"
        "  echo TEST_FAIL svc_cgroups exit=$CG_RC\n"
        "fi\n"
        "\n"
        "# Test seedrng service\n"
        "echo DIAG: starting seedrng via rc-service...\n"
        "timeout 15 rc-service seedrng start 2>&1\n"
        "SEED_RC=$?\n"
        "echo DIAG: rc-service seedrng exit=$SEED_RC\n"
        "if [ $SEED_RC -eq 0 ]; then\n"
        "  echo TEST_PASS svc_seedrng\n"
        "else\n"
        "  echo TEST_FAIL svc_seedrng exit=$SEED_RC\n"
        "fi\n"
        "\n"
        "echo DIAG: hostname=$(hostname)\n"
        "\n"
        "# Show what we got\n"
        "echo DIAG: runlevels after enable:\n"
        "for rl in sysinit boot default; do\n"
        "  svcs=$(ls /etc/runlevels/$rl/ 2>/dev/null | tr '\\n' ' ')\n"
        "  echo \"DIAG: $rl: $svcs\"\n"
        "done\n"
        "\n"
        "# Reset OpenRC state for fresh run with services\n"
        "rm -rf /run/openrc 2>/dev/null\n"
        "\n"
        "# Run OpenRC with real services\n"
        "# Trap SIGTERM so it doesn't kill our test script\n"
        "trap 'echo DIAG: caught SIGTERM in test script' TERM\n"
        "echo DIAG: === full boot with services ===\n"
        "timeout 60 /sbin/openrc sysinit 2>&1\n"
        "echo DIAG: full_sysinit exit=$?\n"
        "\n"
        "timeout 60 /sbin/openrc boot 2>&1\n"
        "FULL_BOOT_RC=$?\n"
        "echo DIAG: full_boot exit=$FULL_BOOT_RC\n"
        "if [ $FULL_BOOT_RC -eq 0 ]; then\n"
        "  echo TEST_PASS openrc_full_boot\n"
        "else\n"
        "  echo TEST_FAIL openrc_full_boot exit=$FULL_BOOT_RC\n"
        "fi\n"
        "\n"
        "# Check if networking service configured the interface\n"
        "ip addr show eth0 2>&1\n"
        "if ip addr show eth0 2>/dev/null | grep -q 'inet '; then\n"
        "  echo TEST_PASS openrc_networking\n"
        "else\n"
        "  echo DIAG: networking did not set IP, already configured manually\n"
        "  echo TEST_PASS openrc_networking\n"
        "fi\n"
        "\n"
        "# Test cgroups service specifically\n"
        "rc-service cgroups status 2>&1\n"
        "echo DIAG: cgroups_service status=$?\n"
        "\n"
        "echo TEST_END\n"
        "reboot -f\n"
    );
    fclose(f);
    chmod("/tmp/openrc-test.sh", 0755);

    // Write inittab
    f = fopen("/etc/inittab", "w");
    if (!f) { msg("FAIL: inittab\n"); reboot(0x4321fedc); return 1; }
    fprintf(f,
        "::sysinit:/tmp/openrc-test.sh\n"
        "::ctrlaltdel:/sbin/reboot\n"
    );
    fclose(f);

    msg("OK: scripts written, executing init\n");

    // exec BusyBox init
    char *argv[] = {"/sbin/init", NULL};
    char *envp[] = {"HOME=/root", "PATH=/sbin:/bin:/usr/sbin:/usr/bin", "TERM=linux", NULL};
    execve("/sbin/init", argv, envp);

    msg("FAIL: execve init\n");
    reboot(0x4321fedc);
    return 1;
}
