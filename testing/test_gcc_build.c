// Test: Install GCC + musl-dev on Alpine, compile and run C programs.
// Validates the build toolchain works on Kevlar.
//
// Build: musl-gcc -static -O2 -o test-gcc-build test_gcc_build.c
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

static void copy_file(const char *src, const char *dst, int mode) {
    int sfd = open(src, O_RDONLY);
    if (sfd < 0) return;
    int dfd = open(dst, O_WRONLY|O_CREAT|O_TRUNC, mode);
    if (dfd >= 0) {
        char buf[4096]; int n;
        while ((n = read(sfd, buf, sizeof(buf))) > 0) write(dfd, buf, n);
        close(dfd);
    }
    close(sfd);
}

int main(void) {
    msg("=== GCC Build Test ===\n");

    // Mount ext4 + essential filesystems
    mkdir("/mnt", 0755);
    mount("tmpfs", "/mnt", "tmpfs", 0, NULL);
    mkdir("/mnt/root", 0755);
    if (mount("none", "/mnt/root", "ext4", 0, NULL) != 0) {
        msg("TEST_FAIL mount_ext4\nTEST_END 0/1\n");
        reboot(0x4321fedc); return 1;
    }
    msg("TEST_PASS mount_ext4\n");

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

    // Copy apk.static
    copy_file("/bin/apk.static", "/mnt/root/sbin/apk.static", 0755);

    // pivot_root
    mkdir("/mnt/root/oldroot", 0755);
    if (syscall(155, "/mnt/root", "/mnt/root/oldroot") != 0) {
        msg("TEST_FAIL pivot_root\n"); reboot(0x4321fedc); return 1;
    }
    chdir("/");
    umount2("/oldroot", MNT_DETACH);
    msg("TEST_PASS pivot_root\n");

    // Write test script
    FILE *f = fopen("/tmp/gcc-test.sh", "w");
    if (!f) { msg("TEST_FAIL script\n"); reboot(0x4321fedc); return 1; }
    fprintf(f,
        "#!/bin/sh\n"
        "exec > /dev/console 2>&1\n"
        "\n"
        "# Network setup\n"
        "ip link set lo up\n"
        "ip link set eth0 up\n"
        "ip addr add 10.0.2.15/24 dev eth0\n"
        "ip route add default via 10.0.2.2\n"
        "echo 'nameserver 10.0.2.3' > /etc/resolv.conf\n"
        "\n"
        "# GCC, musl-dev, make are pre-installed in the Alpine image.\n"
        "# Verify they exist.\n"
        "if [ -x /usr/bin/gcc ] && [ -x /usr/bin/make ]; then\n"
        "  echo TEST_PASS gcc_install\n"
        "else\n"
        "  echo TEST_FAIL gcc_install\n"
        "  echo DIAG: gcc=$(which gcc 2>&1) make=$(which make 2>&1)\n"
        "  echo TEST_END\n"
        "  reboot -f\n"
        "fi\n"
        "\n"
        "# Check gcc is available\n"
        "gcc --version 2>&1 | head -1\n"
        "if [ $? -eq 0 ]; then echo TEST_PASS gcc_version; else echo TEST_FAIL gcc_version; fi\n"
        "\n"
        "# Test 1: Compile minimal program\n"
        "echo 'int main(void) { return 42; }' > /tmp/t.c\n"
        "gcc -o /tmp/t /tmp/t.c 2>&1\n"
        "if [ $? -eq 0 ]; then echo TEST_PASS gcc_compile_minimal; else echo TEST_FAIL gcc_compile_minimal; fi\n"
        "\n"
        "# Test 2: Run compiled binary and check exit code\n"
        "/tmp/t\n"
        "if [ $? -eq 42 ]; then echo TEST_PASS gcc_run_minimal; else echo TEST_FAIL gcc_run_minimal exit=$?; fi\n"
        "\n"
        "# Test 3: Compile hello world with printf\n"
        "cat > /tmp/hello.c << 'CEOF'\n"
        "#include <stdio.h>\n"
        "int main(void) {\n"
        "    printf(\"Hello from Kevlar!\\n\");\n"
        "    return 0;\n"
        "}\n"
        "CEOF\n"
        "gcc -o /tmp/hello /tmp/hello.c 2>&1\n"
        "if [ $? -eq 0 ]; then echo TEST_PASS gcc_compile_hello; else echo TEST_FAIL gcc_compile_hello; fi\n"
        "\n"
        "# Test 4: Run hello world\n"
        "OUTPUT=$(/tmp/hello 2>&1)\n"
        "echo DIAG: hello output: $OUTPUT\n"
        "if [ \"$OUTPUT\" = \"Hello from Kevlar!\" ]; then echo TEST_PASS gcc_run_hello; else echo TEST_FAIL gcc_run_hello; fi\n"
        "\n"
        "# Test 5: Compile with -O2 optimization\n"
        "cat > /tmp/fib.c << 'CEOF'\n"
        "int fib(int n) { return n <= 1 ? n : fib(n-1) + fib(n-2); }\n"
        "int main(void) { return fib(10) == 55 ? 0 : 1; }\n"
        "CEOF\n"
        "gcc -O2 -o /tmp/fib /tmp/fib.c 2>&1\n"
        "if [ $? -eq 0 ]; then echo TEST_PASS gcc_compile_o2; else echo TEST_FAIL gcc_compile_o2; fi\n"
        "/tmp/fib\n"
        "if [ $? -eq 0 ]; then echo TEST_PASS gcc_run_o2; else echo TEST_FAIL gcc_run_o2; fi\n"
        "\n"
        "# Test 6: Compile multi-file with make\n"
        "mkdir -p /tmp/proj\n"
        "cat > /tmp/proj/add.c << 'CEOF'\n"
        "int add(int a, int b) { return a + b; }\n"
        "CEOF\n"
        "cat > /tmp/proj/main.c << 'CEOF'\n"
        "#include <stdio.h>\n"
        "extern int add(int, int);\n"
        "int main(void) {\n"
        "    int result = add(3, 4);\n"
        "    printf(\"3+4=%%d\\n\", result);\n"
        "    return result == 7 ? 0 : 1;\n"
        "}\n"
        "CEOF\n"
        "echo DIAG: gcc search dirs:\n"
        "gcc -print-search-dirs 2>&1 | head -3\n"
        "echo DIAG: gcc multilib:\n"
        "gcc -print-multi-os-directory 2>&1\n"
        "echo DIAG: direct link from /tmp/proj...\n"
        "cd /tmp/proj\n"
        "gcc -c main.c && gcc -c add.c\n"
        "gcc -o prog main.o add.o 2>&1\n"
        "LINK_RC=$?\n"
        "echo DIAG: link exit=$LINK_RC\n"
        "if [ $LINK_RC -eq 0 ]; then\n"
        "  OUTPUT=$(./prog 2>&1)\n"
        "  echo DIAG: output=$OUTPUT\n"
        "fi\n"
        "cd /\n"
        "cat > /tmp/proj/Makefile << 'MEOF'\n"
        "prog: main.o add.o\n"
        "\tgcc -o prog main.o add.o\n"
        "main.o: main.c\n"
        "\tgcc -c main.c\n"
        "add.o: add.c\n"
        "\tgcc -c add.c\n"
        "clean:\n"
        "\trm -f prog *.o\n"
        "MEOF\n"
        "cd /tmp/proj && make 2>&1\n"
        "if [ $? -eq 0 ]; then echo TEST_PASS make_build; else echo TEST_FAIL make_build; fi\n"
        "OUTPUT=$(cd /tmp/proj && ./prog 2>&1)\n"
        "echo DIAG: proj output: $OUTPUT\n"
        "if [ \"$OUTPUT\" = \"3+4=7\" ]; then echo TEST_PASS make_run; else echo TEST_FAIL make_run; fi\n"
        "\n"
        "# Test 7: Compile shared library + link\n"
        "cat > /tmp/mylib.c << 'CEOF'\n"
        "int square(int x) { return x * x; }\n"
        "CEOF\n"
        "cat > /tmp/use_lib.c << 'CEOF'\n"
        "#include <stdio.h>\n"
        "extern int square(int);\n"
        "int main(void) {\n"
        "    printf(\"5^2=%%d\\n\", square(5));\n"
        "    return square(5) == 25 ? 0 : 1;\n"
        "}\n"
        "CEOF\n"
        "gcc -shared -fPIC -o /tmp/libmylib.so /tmp/mylib.c 2>&1\n"
        "if [ $? -eq 0 ]; then echo TEST_PASS gcc_shared_lib; else echo TEST_FAIL gcc_shared_lib; fi\n"
        "gcc -o /tmp/use_lib /tmp/use_lib.c -L/tmp -lmylib 2>&1\n"
        "if [ $? -eq 0 ]; then echo TEST_PASS gcc_link_shared; else echo TEST_FAIL gcc_link_shared; fi\n"
        "LD_LIBRARY_PATH=/tmp /tmp/use_lib 2>&1\n"
        "if [ $? -eq 0 ]; then echo TEST_PASS gcc_run_shared; else echo TEST_FAIL gcc_run_shared; fi\n"
        "\n"
        "echo TEST_END\n"
        "reboot -f\n"
    );
    fclose(f);
    chmod("/tmp/gcc-test.sh", 0755);

    // Write inittab
    f = fopen("/etc/inittab", "w");
    if (!f) { msg("TEST_FAIL inittab\n"); reboot(0x4321fedc); return 1; }
    fprintf(f, "::sysinit:/tmp/gcc-test.sh\n::ctrlaltdel:/sbin/reboot\n");
    fclose(f);

    msg("OK: starting init\n");
    char *argv[] = {"/sbin/init", NULL};
    char *envp[] = {"HOME=/root", "PATH=/sbin:/bin:/usr/sbin:/usr/bin", "TERM=linux", NULL};
    execve("/sbin/init", argv, envp);
    msg("FAIL: execve init\n");
    reboot(0x4321fedc);
    return 1;
}
