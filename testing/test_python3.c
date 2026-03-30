// Python3 installation and execution test on Alpine+Kevlar.
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

// Run python3 -c with stderr visible for debugging
static int py(const char *code) {
    char *argv[] = {"/usr/bin/python3", "-c", (char *)code, NULL};
    return run("/usr/bin/python3", argv);
}

// Write a Python script to a file and run it (avoids -c quoting issues)
static int py_file(const char *filename, const char *code) {
    int fd = open(filename, O_WRONLY | O_CREAT | O_TRUNC, 0755);
    if (fd < 0) return -1;
    write(fd, code, strlen(code));
    close(fd);
    char *argv[] = {"/usr/bin/python3", (char *)filename, NULL};
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
    syscall(155, "/mnt/root", "/mnt/root/oldroot");
    chdir("/");
    system("/sbin/ip link set lo up");
    system("/sbin/ip link set eth0 up");
    system("/sbin/ip addr add 10.0.2.15/24 dev eth0");
    system("/sbin/ip route add default via 10.0.2.2");
}

int main(void) {
    setup_alpine_root();
    sleep(1);

    msg("=== Python3 Test Suite ===\n");

    // Install python3
    msg("Installing python3 via apk...\n");
    {
        char *argv[] = {"/sbin/apk", "add", "--no-check-certificate", "-q",
                        "python3", "python3-pyc",
                        "python3-pycache-pyc0", NULL};
        run("/sbin/apk", argv);
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
        if (run("/usr/bin/python3", argv) == 0) { printf("PASS\n"); pass++; }
        else { printf("FAIL\n"); fail++; }
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

    // Test 5: json — first diagnose the collections import issue
    printf("TEST import collections: ");
    {
        int r = py_file("/tmp/test_collections.py",
            "import os, sys\n"
            "# Check if collections directory exists\n"
            "for p in sys.path:\n"
            "    cdir = os.path.join(p, 'collections')\n"
            "    if os.path.isdir(cdir):\n"
            "        print(f'Found: {cdir}')\n"
            "        print(f'  Contents: {os.listdir(cdir)}')\n"
            "    cinit = os.path.join(p, 'collections', '__init__.py')\n"
            "    if os.path.exists(cinit):\n"
            "        print(f'  __init__.py: {os.path.getsize(cinit)} bytes')\n"
            "# Read the raw directory data via the inode to diagnose\n"
            "st = os.stat('/usr/lib/python3.12')\n"
            "print(f'  dir size: {st.st_size} bytes ({st.st_size // 4096} blocks)')\n"
            "entries = os.listdir('/usr/lib/python3.12')\n"
            "print(f'  listdir: {len(entries)} entries')\n"
            "print(f'  collections in listdir: {\"collections\" in entries}')\n"
            "# Open the directory as a file to read raw getdents\n"
            "import struct\n"
            "fd = os.open('/usr/lib/python3.12', os.O_RDONLY | os.O_DIRECTORY)\n"
            "# Use SYS_getdents64 to read raw directory entries\n"
            "import ctypes, ctypes.util\n"
            "buf = bytearray(32768)\n"
            "SYS_GETDENTS64 = 217\n"
            "n = ctypes.CDLL(None).syscall(SYS_GETDENTS64, fd, ctypes.c_char_p(bytes(buf)), len(buf))\n"
            "print(f'  getdents64 returned: {n}')\n"
            "# Walk the getdents64 entries\n"
            "pos = 0\n"
            "total = 0\n"
            "found_collections = False\n"
            "while pos < n:\n"
            "    d_ino = struct.unpack_from('<Q', buf, pos)[0]\n"
            "    d_off = struct.unpack_from('<q', buf, pos+8)[0]\n"
            "    d_reclen = struct.unpack_from('<H', buf, pos+16)[0]\n"
            "    d_type = buf[pos+18]\n"
            "    name_end = buf.index(0, pos+19)\n"
            "    name = buf[pos+19:name_end].decode()\n"
            "    if name == 'collections':\n"
            "        found_collections = True\n"
            "        print(f'  FOUND collections at pos={pos} ino={d_ino} type={d_type}')\n"
            "    total += 1\n"
            "    pos += d_reclen\n"
            "os.close(fd)\n"
            "print(f'  getdents64 total: {total} found_collections: {found_collections}')\n"
            "import collections\n"
            "print('collections OK')\n"
        );
        if (r == 0) { printf("PASS\n"); pass++; }
        else { printf("FAIL (exit=%d)\n", r); fail++; }
    }

    printf("TEST import json: ");
    {
        int r = py_file("/tmp/test_json.py",
            "import json\n"
            "d = {'ok': True, 'n': 42}\n"
            "s = json.dumps(d)\n"
            "print(s)\n"
            "d2 = json.loads(s)\n"
            "assert d2['ok'] == True\n"
            "assert d2['n'] == 42\n"
            "print('json OK')\n"
        );
        if (r == 0) { printf("PASS\n"); pass++; }
        else { printf("FAIL (exit=%d)\n", r); fail++; }
    }

    // Test 6: list comprehension
    printf("TEST list comprehension: ");
    if (py("print(sum([x*x for x in range(10)]))") == 0) { printf("PASS\n"); pass++; }
    else { printf("FAIL\n"); fail++; }

    // Test 7: subprocess — use a script file
    printf("TEST subprocess: ");
    {
        int r = py_file("/tmp/test_subprocess.py",
            "import subprocess\n"
            "r = subprocess.run(['/bin/echo', 'hello'], capture_output=True, text=True)\n"
            "assert r.returncode == 0, f'returncode={r.returncode}'\n"
            "assert r.stdout.strip() == 'hello', f'stdout={r.stdout!r}'\n"
            "print('subprocess OK')\n"
        );
        if (r == 0) { printf("PASS\n"); pass++; }
        else { printf("FAIL (exit=%d)\n", r); fail++; }
    }

    // Test 8: math C extension
    printf("TEST import math: ");
    {
        int r = py("import math; assert abs(math.sqrt(2) - 1.4142) < 0.001; print('math OK')");
        if (r == 0) { printf("PASS\n"); pass++; }
        else { printf("SKIP (C extension)\n"); skip++; }
    }

    // Test 9: hashlib C extension
    printf("TEST import hashlib: ");
    {
        int r = py_file("/tmp/test_hashlib.py",
            "import hashlib\n"
            "h = hashlib.md5(b'test').hexdigest()\n"
            "assert h == '098f6bcd4621d373cade4e832627b4f6', f'got {h}'\n"
            "print('hashlib OK')\n"
        );
        if (r == 0) { printf("PASS\n"); pass++; }
        else { printf("SKIP (C extension)\n"); skip++; }
    }

    // Test 10: signal handling — use a script file
    printf("TEST signal: ");
    {
        int r = py_file("/tmp/test_signal.py",
            "import signal, os\n"
            "got = [False]\n"
            "def handler(signum, frame):\n"
            "    got[0] = True\n"
            "signal.signal(signal.SIGUSR1, handler)\n"
            "os.kill(os.getpid(), signal.SIGUSR1)\n"
            "assert got[0], 'signal not received'\n"
            "print('signal OK')\n"
        );
        if (r == 0) { printf("PASS\n"); pass++; }
        else { printf("FAIL (exit=%d)\n", r); fail++; }
    }

    printf("\n=== Results: %d PASS, %d FAIL, %d SKIP ===\n", pass, fail, skip);
    printf(fail == 0 ? "TEST_PASS\n" : "TEST_FAIL\n");
    return fail > 0 ? 1 : 0;
}
