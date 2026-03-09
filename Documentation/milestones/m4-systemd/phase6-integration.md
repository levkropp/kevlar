# Phase 6: Integration Testing

**Goal:** Boot a real systemd binary on Kevlar, diagnose failures, and fix them
iteratively until we reach multi-user.target.

**Prerequisite:** Phases 1-5 complete.

## Strategy

systemd is ~1.5M lines of C. We will not read it all. Instead:

1. Build a minimal systemd+dbus for musl (or use a prebuilt one)
2. Boot it as PID 1 on Kevlar
3. Watch it crash
4. Read the crash, implement/fix the missing piece
5. Repeat until it doesn't crash

This phase is inherently iterative. The plan below covers the expected
failure modes and likely stub requirements.

## Building a Minimal systemd

Options (in order of preference):

1. **Chimera Linux systemd** — Chimera Linux builds systemd against musl.
   They maintain patches for musl compatibility. This is the most promising
   path since our userspace is musl-based.

2. **systemd-nspawn approach** — Build systemd in a Debian chroot, copy
   the binaries. Requires glibc ld-linux.so in the initramfs.

3. **Minimal custom init** — Write a C program that exercises the same
   syscalls as systemd (epoll + signalfd + timerfd + mount + fork/exec)
   without being actual systemd. Good for pre-testing Phases 1-5.

**Recommendation:** Start with option 3 (custom mini-init) during Phases 1-5
to validate each subsystem. Switch to real systemd (option 1 or 2) when all
phases are complete.

## Expected Failure Modes

### Round 1: Missing syscalls

systemd calls many syscalls beyond our explicit list. These will return
ENOSYS and systemd may or may not handle the error. Expected stubs needed:

| Syscall | Likely behavior | Our response |
|---------|----------------|--------------|
| `inotify_init1` (294) | Watch unit file dirs | Return ENOSYS (systemd has fallback) |
| `name_to_handle_at` (303) | Device tracking | Return ENOSYS (systemd copes) |
| `fanotify_init` (300) | File access monitoring | Return ENOSYS |
| `personality` (135) | Execution domain | Return 0 (stub) |
| `sched_setaffinity` (203) | CPU pinning | Return 0 (stub, single CPU) |
| `clock_getres` (229) | Clock resolution query | Return {0, 1} |
| `fstatfs` (138) | FS type checking | Return reasonable defaults |
| `getdents` (78) | Old readdir | Redirect to getdents64 |
| `ppoll` (271) | Enhanced poll | We have this (partial) |
| `prlimit64` (302) | Resource limits | We have this (partial) |
| `setrlimit` (160) | Set resource limits | Return 0 (stub) |

### Round 2: /proc gaps

systemd reads many /proc files. Missing ones will cause fallback behavior
or errors:

| Path | Why systemd reads it | Priority |
|------|---------------------|----------|
| `/proc/1/environ` | Own environment | Medium |
| `/proc/1/cgroup` | cgroup membership | Stub (empty) |
| `/proc/1/mountinfo` | Detailed mount info | Medium |
| `/proc/sys/kernel/random/boot_id` | Unique boot identifier | Return UUID |
| `/proc/sys/kernel/hostname` | System hostname | Return "kevlar" |
| `/proc/sys/kernel/osrelease` | Kernel version | Return our version |

### Round 3: D-Bus bootstrap

systemd starts dbus-daemon (or dbus-broker) and communicates via AF_UNIX
sockets with SCM_RIGHTS. This requires Phase 3 to be solid. Expected issues:

- Socket creation/binding path issues
- cmsg parsing edge cases
- SOCK_CLOEXEC propagation

### Round 4: Service management

Once systemd's main loop runs, it tries to start services. Each service is
a fork+exec with various namespace/cgroup/capability setup. Expected issues:

- Services expecting cgroup controllers
- Socket activation (systemd passes pre-opened sockets to services)
- Watchdog timer integration (timerfd-based)

## Minimal Init Test Program

Before attempting real systemd, validate all subsystems with this:

```c
// integration_tests/mini_systemd.c
// Exercises the same codepaths as systemd PID 1 initialization.

#include <sys/epoll.h>
#include <sys/signalfd.h>
#include <sys/timerfd.h>
#include <sys/eventfd.h>
#include <sys/mount.h>

int main() {
    // Phase 4: Mount pseudo-filesystems
    mount("proc", "/proc", "proc", 0, NULL);
    mount("sysfs", "/sys", "sysfs", 0, NULL);
    mount("tmpfs", "/run", "tmpfs", 0, NULL);

    // Phase 1: Create epoll
    int epfd = epoll_create1(EPOLL_CLOEXEC);

    // Phase 2: Set up event sources
    sigset_t mask;
    sigfillset(&mask);
    sigprocmask(SIG_BLOCK, &mask, NULL);
    int sfd = signalfd(-1, &mask, SFD_NONBLOCK | SFD_CLOEXEC);

    int tfd = timerfd_create(CLOCK_MONOTONIC, TFD_CLOEXEC);
    struct itimerspec its = { .it_value = { .tv_sec = 1 } };
    timerfd_settime(tfd, 0, &its, NULL);

    int efd = eventfd(0, EFD_CLOEXEC | EFD_NONBLOCK);

    // Add all to epoll
    struct epoll_event ev;
    ev.events = EPOLLIN;
    ev.data.fd = sfd; epoll_ctl(epfd, EPOLL_CTL_ADD, sfd, &ev);
    ev.data.fd = tfd; epoll_ctl(epfd, EPOLL_CTL_ADD, tfd, &ev);
    ev.data.fd = efd; epoll_ctl(epfd, EPOLL_CTL_ADD, efd, &ev);

    // Phase 5: prctl
    prctl(PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0);
    prctl(PR_SET_NAME, "mini-systemd", 0, 0, 0);

    // Phase 3: Create notify socket
    int nfd = socket(AF_UNIX, SOCK_DGRAM | SOCK_CLOEXEC, 0);
    struct sockaddr_un addr = { .sun_family = AF_UNIX };
    strcpy(addr.sun_path, "/run/systemd/notify");
    mkdir("/run/systemd", 0755);
    bind(nfd, (struct sockaddr *)&addr, sizeof(addr));
    ev.data.fd = nfd; epoll_ctl(epfd, EPOLL_CTL_ADD, nfd, &ev);

    // Fork a test service
    pid_t child = fork();
    if (child == 0) {
        printf("service: running\n");
        _exit(0);
    }

    // Main loop: wait for events
    struct epoll_event events[8];
    for (int iter = 0; iter < 5; iter++) {
        int n = epoll_wait(epfd, events, 8, 500);
        for (int i = 0; i < n; i++) {
            if (events[i].data.fd == sfd) {
                struct signalfd_siginfo si;
                read(sfd, &si, sizeof(si));
                if (si.ssi_signo == SIGCHLD) {
                    waitpid(-1, NULL, WNOHANG);
                }
            } else if (events[i].data.fd == tfd) {
                uint64_t exp;
                read(tfd, &exp, sizeof(exp));
            }
        }
    }

    printf("TEST_PASS mini_systemd\n");
    return 0;
}
```

## Success Criteria (Progressive)

1. **mini_systemd test passes** — All Phases 1-5 working together
2. **systemd boots without panic** — May print errors but doesn't crash
3. **systemd reaches default.target** — Basic service management works
4. **`systemctl status` works** — D-Bus communication functional
5. **A custom .service starts** — Fork+exec+monitor cycle complete

## Timeline Estimate

Phase 6 is open-ended by nature. Budget as much time as Phases 1-5 combined
for integration debugging. The mini_systemd test should pass quickly; real
systemd will take iterative fixing.
