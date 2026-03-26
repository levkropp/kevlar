// Diagnostic: run openrc and capture crash details.
// Runs openrc in a child process with SIGSEGV/SIGILL signal handler
// that prints register state and the bytes at the faulting IP.
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <signal.h>
#include <ucontext.h>
#include <unistd.h>
#include <sys/wait.h>
#include <sys/mman.h>
#include <fcntl.h>
#include <dlfcn.h>
#include <link.h>

static void crash_handler(int sig, siginfo_t *info, void *ctx) {
    ucontext_t *uc = (ucontext_t *)ctx;
    unsigned long rip = uc->uc_mcontext.gregs[REG_RIP];
    unsigned long rsp = uc->uc_mcontext.gregs[REG_RSP];
    unsigned long rax = uc->uc_mcontext.gregs[REG_RAX];
    unsigned long rbx = uc->uc_mcontext.gregs[REG_RBX];

    char buf[512];
    int n = snprintf(buf, sizeof(buf),
        "\n=== OPENRC CRASH DIAGNOSTIC ===\n"
        "Signal: %d (%s)\n"
        "RIP: 0x%lx\n"
        "RSP: 0x%lx\n"
        "RAX: 0x%lx  RBX: 0x%lx\n",
        sig, sig == SIGILL ? "SIGILL" : sig == SIGSEGV ? "SIGSEGV" : "?",
        rip, rsp, rax, rbx);
    write(2, buf, n);

    // Dump bytes at RIP (if accessible)
    unsigned char *ip = (unsigned char *)rip;
    n = snprintf(buf, sizeof(buf), "Bytes at RIP:");
    for (int i = -8; i < 16; i++) {
        // Try to read each byte safely
        n += snprintf(buf + n, sizeof(buf) - n, "%s%02x",
                      i == 0 ? " [" : " ", ip[i]);
        if (i == 0) n += snprintf(buf + n, sizeof(buf) - n, "]");
    }
    n += snprintf(buf + n, sizeof(buf) - n, "\n");
    write(2, buf, n);

    // Print loaded libraries by reading /proc/self/maps
    int maps_fd = open("/proc/self/maps", O_RDONLY);
    if (maps_fd >= 0) {
        char maps[4096];
        int mr = read(maps_fd, maps, sizeof(maps) - 1);
        if (mr > 0) {
            maps[mr] = 0;
            // Find the line containing RIP
            char *line = maps;
            while (line && *line) {
                char *nl = strchr(line, '\n');
                if (nl) *nl = 0;
                // Parse start-end of this mapping
                unsigned long start = 0, end = 0;
                sscanf(line, "%lx-%lx", &start, &end);
                if (rip >= start && rip < end) {
                    n = snprintf(buf, sizeof(buf), "RIP in mapping: %s\n", line);
                    write(2, buf, n);
                }
                if (nl) { *nl = '\n'; line = nl + 1; } else break;
            }
            // Print all mappings
            write(2, "=== /proc/self/maps ===\n", 24);
            write(2, maps, mr);
            write(2, "=== end maps ===\n", 17);
        }
        close(maps_fd);
    }

    write(2, "=== END CRASH DIAGNOSTIC ===\n", 29);
    _exit(128 + sig);
}

// Callback for dl_iterate_phdr — print all loaded shared objects
static int phdr_callback(struct dl_phdr_info *info, size_t size, void *data) {
    (void)size; (void)data;
    if (info->dlpi_name && info->dlpi_name[0]) {
        char buf[256];
        int n = snprintf(buf, sizeof(buf), "DIAG loaded: base=0x%lx %s\n",
                         (unsigned long)info->dlpi_addr, info->dlpi_name);
        write(1, buf, n);
    }
    return 0;
}

int main(int argc, char **argv) {
    (void)argc; (void)argv;

    printf("=== OpenRC Crash Diagnostic ===\n");
    fflush(stdout);

    // Install crash handler
    struct sigaction sa = {0};
    sa.sa_sigaction = crash_handler;
    sa.sa_flags = SA_SIGINFO;
    sigaction(SIGSEGV, &sa, NULL);
    sigaction(SIGILL, &sa, NULL);
    sigaction(SIGBUS, &sa, NULL);
    sigaction(SIGFPE, &sa, NULL);

    // Print loaded libraries
    dl_iterate_phdr(phdr_callback, NULL);
    fflush(stdout);

    printf("DIAG: exec openrc sysinit\n");
    fflush(stdout);

    // Fork and exec openrc
    pid_t pid = fork();
    if (pid == 0) {
        // Child — exec openrc with crash handler inherited
        char *argv[] = {"/sbin/openrc", "sysinit", NULL};
        char *envp[] = {
            "PATH=/usr/sbin:/usr/bin:/sbin:/bin",
            "HOME=/root",
            "TERM=vt100",
            NULL
        };
        execve("/sbin/openrc", argv, envp);
        perror("execve failed");
        _exit(127);
    }

    int status;
    waitpid(pid, &status, 0);
    printf("DIAG: openrc exited status=%d signal=%d\n",
           WIFEXITED(status) ? WEXITSTATUS(status) : -1,
           WIFSIGNALED(status) ? WTERMSIG(status) : 0);
    fflush(stdout);

    printf("=== Done ===\n");
    return 0;
}
