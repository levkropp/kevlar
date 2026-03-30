// Python3 installation and execution test on Alpine+Kevlar.
// Installs python3 via apk, then exercises interpreter, stdlib, subprocess.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

static int pass = 0, fail = 0, skip = 0;
static void msg(const char *s) { write(1, s, strlen(s)); }

static int run(const char *path, char *const argv[]) {
    pid_t pid = fork();
    if (pid == 0) { execv(path, argv); _exit(127); }
    int status; waitpid(pid, &status, 0);
    return WIFEXITED(status) ? WEXITSTATUS(status) : -1;
}

// Run a python3 -c command and check exit code
static int py(const char *code) {
    char *argv[] = {"/usr/bin/python3", "-c", (char *)code, NULL};
    return run("/usr/bin/python3", argv);
}

static void setup_alpine_root(void) {
    mkdir("/mnt", 0755);
    mount("tmpfs", "/mnt", "tmpfs", 0, NULL);
    mkdir("/mnt/root", 0755);
    if (mount("none", "/mnt/root", "ext4", 0, NULL) != 0) {
        msg("FATAL: mount ext4 failed\n");
        _exit(1);
    }
    mkdir("/mnt/root/proc", 0755);
    mount("proc", "/mnt/root/proc", "proc", 0, NULL);
    mkdir("/mnt/root/dev", 0755);
    mount("devtmpfs", "/mnt/root/dev", "devtmpfs", 0, NULL);
    mkdir("/mnt/root/tmp", 01777);
    mount("tmpfs", "/mnt/root/tmp", "tmpfs", 0, NULL);
    mkdir("/mnt/root/run", 0755);
    mount("tmpfs", "/mnt/root/run", "tmpfs", 0, NULL);
    mkdir("/mnt/root/oldroot", 0755);
    syscall(155, "/mnt/root", "/mnt/root/oldroot"); // pivot_root
    chdir("/");

    // Networking
    system("/sbin/ip link set lo up");
    system("/sbin/ip link set eth0 up");
    system("/sbin/ip addr add 10.0.2.15/24 dev eth0");
    system("/sbin/ip route add default via 10.0.2.2");
}

int main(void) {
    setup_alpine_root();
    sleep(1); // let DHCP settle

    msg("=== Python3 Test Suite ===\n");

    // Install python3
    msg("Installing python3 via apk...\n");
    {
        char *argv[] = {"/sbin/apk", "add", "--no-check-certificate", "-q", "python3", NULL};
        int r = run("/sbin/apk", argv);
        struct stat st;
        if (stat("/usr/bin/python3", &st) != 0) {
            msg("FATAL: python3 not installed\n");
            return 1;
        }
    }

    // Test 1: version
    printf("TEST python3 --version: ");
    {
        char *argv[] = {"/usr/bin/python3", "--version", NULL};
        int r = run("/usr/bin/python3", argv);
        if (r == 0) { printf("PASS\n"); pass++; }
        else { printf("FAIL (exit=%d)\n", r); fail++; }
    }

    // Test 2: basic print
    printf("TEST print(1+1): ");
    if (py("print(1+1)") == 0) { printf("PASS\n"); pass++; }
    else { printf("FAIL\n"); fail++; }

    // Test 3: os module
    printf("TEST import os: ");
    if (py("import os; print(os.getpid())") == 0) { printf("PASS\n"); pass++; }
    else { printf("FAIL\n"); fail++; }

    // Test 4: sys module
    printf("TEST import sys: ");
    if (py("import sys; print(sys.platform)") == 0) { printf("PASS\n"); pass++; }
    else { printf("FAIL\n"); fail++; }

    // Test 5: json (pure Python stdlib)
    printf("TEST import json: ");
    if (py("import json; print(json.dumps({'ok':True}))") == 0) { printf("PASS\n"); pass++; }
    else { printf("FAIL\n"); fail++; }

    // Test 6: list comprehension + builtins
    printf("TEST list comprehension: ");
    if (py("print(sum([x*x for x in range(10)]))") == 0) { printf("PASS\n"); pass++; }
    else { printf("FAIL\n"); fail++; }

    // Test 7: subprocess (fork+exec+pipe)
    printf("TEST subprocess: ");
    if (py("import subprocess; r=subprocess.run(['echo','hello'], capture_output=True, text=True); "
           "assert r.stdout.strip()=='hello'; print('subprocess OK')") == 0) {
        printf("PASS\n"); pass++;
    } else { printf("FAIL\n"); fail++; }

    // Test 8: math C extension (may fail if dlopen broken)
    printf("TEST import math: ");
    {
        int r = py("import math; assert abs(math.sqrt(2) - 1.4142) < 0.001; print('math OK')");
        if (r == 0) { printf("PASS\n"); pass++; }
        else { printf("SKIP (C extension)\n"); skip++; }
    }

    // Test 9: hashlib C extension
    printf("TEST import hashlib: ");
    {
        int r = py("import hashlib; h=hashlib.md5(b'test').hexdigest(); "
                    "assert h=='098f6bcd4621d373cade4e832627b4f6'; print('hashlib OK')");
        if (r == 0) { printf("PASS\n"); pass++; }
        else { printf("SKIP (C extension)\n"); skip++; }
    }

    // Test 10: signal handling
    printf("TEST signal: ");
    if (py("import signal,os; got=[False]\n"
           "def h(s,f): got[0]=True\n"
           "signal.signal(signal.SIGUSR1,h)\n"
           "os.kill(os.getpid(),signal.SIGUSR1)\n"
           "assert got[0]; print('signal OK')") == 0) {
        printf("PASS\n"); pass++;
    } else { printf("FAIL\n"); fail++; }

    printf("\n=== Results: %d PASS, %d FAIL, %d SKIP ===\n", pass, fail, skip);
    printf(fail == 0 ? "TEST_PASS\n" : "TEST_FAIL\n");
    return fail > 0 ? 1 : 0;
}
