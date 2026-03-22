// Like boot_alpine.c but runs gcc test after pivot_root instead of init.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <string.h>
#include <stdio.h>

static void msg(const char *s) { write(1, s, strlen(s)); }

int main(void) {
    msg("kevlar: gcc test shim starting\n");
    mkdir("/mnt", 0755);
    mount("tmpfs", "/mnt", "tmpfs", 0, NULL);
    mkdir("/mnt/root", 0755);
    if (mount("none", "/mnt/root", "ext4", 0, NULL) != 0) {
        msg("kevlar: mount ext4 failed\n");
        return 1;
    }
    mkdir("/mnt/root/proc", 0755);
    mount("proc", "/mnt/root/proc", "proc", 0, NULL);
    mkdir("/mnt/root/dev", 0755);
    mkdir("/mnt/root/tmp", 01777);
    mount("tmpfs", "/mnt/root/tmp", "tmpfs", 0, NULL);
    mkdir("/mnt/root/oldroot", 0755);
    if (syscall(155, "/mnt/root", "/mnt/root/oldroot") != 0) {
        msg("kevlar: pivot_root failed\n");
        return 1;
    }
    chdir("/");
    msg("kevlar: running gcc test\n");
    char *argv[] = { "/bin/sh", "-c",
        // Test 1: compile minimal program
        "echo 'int main(){return 42;}' > /root/t.c; "
        "gcc -o /root/t /root/t.c 2>&1; "
        "echo gcc1=$?; "
        // Test 2: run the compiled binary
        "chmod +x /root/t; "
        "/root/t; echo run1=$?; "
        // Test 3: compile and run hello world with printf
        "echo '#include <stdio.h>' > /root/hello.c; "
        "echo 'int main(){printf(\"Hello from Kevlar!\\n\");return 0;}' >> /root/hello.c; "
        "gcc -o /root/hello /root/hello.c 2>&1; "
        "echo gcc2=$?; "
        "chmod +x /root/hello; "
        "/root/hello; echo run2=$?; "
        // Test 4: compile with -O2
        "echo 'int fib(int n){return n<=1?n:fib(n-1)+fib(n-2);}' > /root/fib.c; "
        "echo 'int main(){return fib(10)==55?0:1;}' >> /root/fib.c; "
        "gcc -O2 -o /root/fib /root/fib.c 2>&1; "
        "echo gcc3=$?; "
        "chmod +x /root/fib; "
        "/root/fib; echo run3=$?; "
        "echo DONE; poweroff -f",
        NULL };
    char *envp[] = { "HOME=/root", "PATH=/usr/sbin:/usr/bin:/sbin:/bin", "TERM=vt100", NULL };
    execve("/bin/sh", argv, envp);
    msg("kevlar: exec sh failed\n");
    return 1;
}
