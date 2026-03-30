// Python3 installation and execution test on Alpine+Kevlar.
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

static int pass = 0, fail = 0, skip = 0;

static int run(const char *path, char *const argv[]) {
    pid_t pid = fork();
    if (pid == 0) { execv(path, argv); _exit(127); }
    int status; waitpid(pid, &status, 0);
    return WIFEXITED(status) ? WEXITSTATUS(status) : -1;
}

static int py(const char *code) {
    char *argv[] = {"/usr/bin/python3", "-c", (char *)code, NULL};
    return run("/usr/bin/python3", argv);
}

static int py_file(const char *filename, const char *code) {
    int fd = open(filename, O_WRONLY | O_CREAT | O_TRUNC, 0755);
    if (fd < 0) return -1;
    write(fd, code, strlen(code));
    close(fd);
    char *argv[] = {"/usr/bin/python3", (char *)filename, NULL};
    return run("/usr/bin/python3", argv);
}

static void setup(void) {
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

int main(void) {
    setup();
    sleep(1);
    printf("=== Python3 Test Suite ===\n");

    // Install
    printf("Installing python3...\n");
    char *apk[] = {"/sbin/apk", "add", "--no-check-certificate", "-q", "python3", NULL};
    run("/sbin/apk", apk);
    struct stat st;
    if (stat("/usr/bin/python3", &st) != 0) { printf("FATAL\n"); return 1; }

    printf("TEST python3 --version: ");
    { char *a[] = {"/usr/bin/python3", "--version", NULL};
      if (run("/usr/bin/python3", a) == 0) { printf("PASS\n"); pass++; } else { printf("FAIL\n"); fail++; } }

    printf("TEST print(1+1): ");
    if (py("print(1+1)") == 0) { printf("PASS\n"); pass++; } else { printf("FAIL\n"); fail++; }

    printf("TEST import os: ");
    if (py("import os; print(os.getpid())") == 0) { printf("PASS\n"); pass++; } else { printf("FAIL\n"); fail++; }

    printf("TEST import sys: ");
    if (py("import sys; print(sys.platform)") == 0) { printf("PASS\n"); pass++; } else { printf("FAIL\n"); fail++; }

    printf("TEST import collections: ");
    if (py("from collections import namedtuple; T=namedtuple('T',['x']); print(T(1))") == 0)
    { printf("PASS\n"); pass++; } else { printf("FAIL\n"); fail++; }

    printf("TEST import json: ");
    { int r = py_file("/tmp/t.py",
        "import json\n"
        "d = {'ok': True, 'n': 42}\n"
        "s = json.dumps(d)\n"
        "d2 = json.loads(s)\n"
        "assert d2['ok'] == True\n"
        "print('json OK')\n");
      if (r == 0) { printf("PASS\n"); pass++; } else { printf("FAIL (exit=%d)\n", r); fail++; } }

    printf("TEST list comprehension: ");
    if (py("print(sum([x*x for x in range(10)]))") == 0) { printf("PASS\n"); pass++; } else { printf("FAIL\n"); fail++; }

    printf("TEST subprocess: ");
    { int r = py_file("/tmp/t.py",
        "import subprocess\n"
        "r = subprocess.run(['/bin/echo', 'hello'], capture_output=True, text=True)\n"
        "assert r.returncode == 0\n"
        "assert r.stdout.strip() == 'hello'\n"
        "print('subprocess OK')\n");
      if (r == 0) { printf("PASS\n"); pass++; } else { printf("FAIL (exit=%d)\n", r); fail++; } }

    printf("TEST import math: ");
    { int r = py("import math; assert abs(math.sqrt(2)-1.4142)<0.001; print('math OK')");
      if (r == 0) { printf("PASS\n"); pass++; } else { printf("SKIP\n"); skip++; } }

    printf("TEST import hashlib: ");
    { int r = py_file("/tmp/t.py",
        "import hashlib\n"
        "h = hashlib.md5(b'test').hexdigest()\n"
        "assert h == '098f6bcd4621d373cade4e832627b4f6'\n"
        "print('hashlib OK')\n");
      if (r == 0) { printf("PASS\n"); pass++; } else { printf("SKIP\n"); skip++; } }

    printf("TEST signal: ");
    { int r = py_file("/tmp/t.py",
        "import signal, os\n"
        "got = [False]\n"
        "def handler(signum, frame):\n"
        "    got[0] = True\n"
        "signal.signal(signal.SIGUSR1, handler)\n"
        "os.kill(os.getpid(), signal.SIGUSR1)\n"
        "assert got[0]\n"
        "print('signal OK')\n");
      if (r == 0) { printf("PASS\n"); pass++; } else { printf("FAIL (exit=%d)\n", r); fail++; } }

    printf("\n=== Results: %d PASS, %d FAIL, %d SKIP ===\n", pass, fail, skip);
    printf(fail == 0 ? "TEST_PASS\n" : "TEST_FAIL\n");
    return fail > 0 ? 1 : 0;
}
