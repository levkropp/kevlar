// LD_PRELOAD library: installs a SIGILL/SIGSEGV handler that prints
// register state and code bytes before exiting.
// gcc -shared -fPIC -o crash_handler.so crash_handler.c
#define _GNU_SOURCE
#include <signal.h>
#include <ucontext.h>
#include <unistd.h>
#include <stdio.h>
#include <string.h>
#include <fcntl.h>

static void crash_handler(int sig, siginfo_t *info, void *ctx) {
    ucontext_t *uc = (ucontext_t *)ctx;
    unsigned long rip = uc->uc_mcontext.gregs[REG_RIP];
    unsigned long rsp = uc->uc_mcontext.gregs[REG_RSP];
    unsigned long rax = uc->uc_mcontext.gregs[REG_RAX];
    unsigned long rbx = uc->uc_mcontext.gregs[REG_RBX];
    unsigned long rcx = uc->uc_mcontext.gregs[REG_RCX];
    unsigned long rdx = uc->uc_mcontext.gregs[REG_RDX];
    unsigned long rdi = uc->uc_mcontext.gregs[REG_RDI];
    unsigned long rsi = uc->uc_mcontext.gregs[REG_RSI];

    char buf[1024];
    int n = snprintf(buf, sizeof(buf),
        "\n=== CRASH IN PID %d ===\n"
        "Signal: %d (%s) si_addr=%p\n"
        "RIP: 0x%lx  RSP: 0x%lx\n"
        "RAX: 0x%lx  RBX: 0x%lx\n"
        "RCX: 0x%lx  RDX: 0x%lx\n"
        "RDI: 0x%lx  RSI: 0x%lx\n",
        getpid(),
        sig, sig == 4 ? "SIGILL" : sig == 11 ? "SIGSEGV" : "?",
        info->si_addr,
        rip, rsp, rax, rbx, rcx, rdx, rdi, rsi);
    write(1, buf, n);

    // Dump bytes at RIP
    unsigned char *ip = (unsigned char *)rip;
    n = snprintf(buf, sizeof(buf), "Code at RIP-8:");
    for (int i = -8; i < 24; i++) {
        if (i == 0) n += snprintf(buf + n, sizeof(buf) - n, " [");
        n += snprintf(buf + n, sizeof(buf) - n, "%02x ", ip[i]);
        if (i == 0) n += snprintf(buf + n, sizeof(buf) - n, "] ");
    }
    n += snprintf(buf + n, sizeof(buf) - n, "\n");
    write(1, buf, n);

    // Print /proc/self/maps
    int fd = open("/proc/self/maps", O_RDONLY);
    if (fd >= 0) {
        write(1, "=== /proc/self/maps ===\n", 24);
        char mbuf[4096];
        int r;
        while ((r = read(fd, mbuf, sizeof(mbuf))) > 0)
            write(1, mbuf, r);
        close(fd);
    }

    write(1, "=== END CRASH ===\n", 18);
    _exit(128 + sig);
}

__attribute__((constructor))
static void install_crash_handler(void) {
    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_sigaction = crash_handler;
    sa.sa_flags = SA_SIGINFO;
    sigaction(SIGSEGV, &sa, NULL);
    sigaction(SIGILL, &sa, NULL);
    sigaction(SIGBUS, &sa, NULL);
}
