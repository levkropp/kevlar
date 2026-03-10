# Compatibility with Linux kernel

## Kernel Modules
Not supported.

## libc
- musl: supported (static and dynamic linking; PIE executables with ld-musl interpreter)
- glibc: *not* yet supported (requires clone, robust futex, and other unimplemented features)

## Compatibility Milestones

| Milestone | Syscalls Needed | Currently Have | Status |
|-----------|----------------|----------------|--------|
| M1: Static Busybox | ~50 | 79 | **Complete** — BusyBox boots, shell works |
| M1.5: ARM64 | -- | 79 | **Complete** — BusyBox boots on QEMU virt (cortex-a72) |
| M2: Dynamic linking (ld-linux.so) | ~55 | 83 | **Complete** — PIE + musl dynamic linker works |
| M3: Terminal, Job Control, Bash | ~80 | 103 | **Complete** — terminal, job control, symlinks, clone, *at syscalls |
| M4: systemd-compatible PID 1 | ~110 | 107 | **Complete** — epoll, unix sockets, mount, caps, 15/15 integration tests |
| M5: Persistent Storage | ~120 | 107 | In Progress — statx/inotify/procfs done; VirtIO block + ext2 planned |
| M6: Full networking (SSH, DNS) | ~130 | 107 | Planned |
| M7: Container runtime | ~145 | 107 | Planned |
| M8: Kubuntu 24.04 desktop | ~170 | 107 | Planned |

## System Calls

### Implementation Status Key
- **Full:** All features are implemented.
- **Partial:** Core functionality works but some flags/modes are missing.
- **Stub:** Returns success (0) without doing real work.
- **Unimplemented:** Returns `ENOSYS`.

### Milestone Key
- **M1** = Static Busybox
- **M2** = Dynamic linking
- **M3** = Coreutils + Bash
- **M4** = systemd
- **M5** = apt/dpkg
- **M6** = Full networking
- **M7** = Container runtime
- **M8** = Desktop

### Reference Key
- **FreeBSD** = Reference FreeBSD's linuxulator (sys/compat/linux/) and kernel (BSD-2-Clause) — battle-tested Linux syscall semantics
- **Own** = Implement ourselves (no suitable permissive reference)

<!-- Tip: Use this VSCode plugin to edit this table: https://marketplace.visualstudio.com/items?itemName=darkriszty.markdown-table-prettify -->

| No  | Name                   | Status        | Milestone | Reference | Notes                                                  |
|-----|------------------------|---------------|-----------|-----------|--------------------------------------------------------|
| 0   | read                   | Partial       | M1        |           | Implemented                                            |
| 1   | write                  | Partial       | M1        |           | Implemented                                            |
| 2   | open                   | Partial       | M1        |           | Implemented; modern code uses openat(257) instead      |
| 3   | close                  | Full          | M1        |           |                                                        |
| 4   | stat                   | Partial       | M1        |           | Implemented; modern code uses newfstatat(262) instead  |
| 5   | fstat                  | Partial       | M1        |           | Implemented                                            |
| 6   | lstat                  | Partial       | M1        |           | Implemented                                            |
| 7   | poll                   | Partial       | M1        |           | Implemented                                            |
| 8   | lseek                  | Full          | M1        | FreeBSD   | SEEK_SET/CUR/END              |
| 9   | mmap                   | Full          | M1        | FreeBSD   | MAP_FIXED unmaps existing, prot flags, NX bit support  |
| 10  | mprotect               | Full          | M1        | FreeBSD   | VMA splitting, PTE update, TLB flush  |
| 11  | munmap                 | Full          | M1        | FreeBSD   | VMA removal/splitting, page freeing   |
| 12  | brk                    | Full          | M1        |           |                                                        |
| 13  | rt_sigaction           | Partial       | M1        | Own       | oldact return implemented; sa_flags/sa_mask partial    |
| 14  | rt_sigprocmask         | Full          | M1        |           |                                                        |
| 15  | rt_sigreturn           | Full          | M1        |           |                                                        |
| 16  | ioctl                  | Partial       | M1        | FreeBSD   | TTY ioctls; needs expansion per device                 |
| 17  | pread64                | Full          | M2        | FreeBSD   | Read at offset without changing file position           |
| 18  | pwrite64               | Full          | M3        | Own       | Write at offset without changing file position          |
| 19  | readv                  | Full          | M3        | Own       | Scatter-gather read                                    |
| 20  | writev                 | Full          | M1        |           |                                                        |
| 21  | access                 | Full          | M1        | FreeBSD   | Path resolution               |
| 22  | pipe                   | Full          | M1        |           |                                                        |
| 23  | select                 | Full          | M1        |           | Fixed: writefds uses POLLOUT, errorfds supported       |
| 24  | sched_yield            | Full          | M1        |           | Calls process::switch()                                |
| 25  | mremap                 | Unimplemented | M2        | FreeBSD   | glibc malloc uses this for realloc                     |
| 26  | msync                  | Unimplemented | M5        | FreeBSD   |                                                        |
| 27  | mincore                | Unimplemented | M8        | FreeBSD   |                                                        |
| 28  | madvise                | Stub          | M2        | FreeBSD   | Returns 0; MADV_DONTNEED needs real impl later         |
| 29  | shmget                 | Unimplemented | M8        | FreeBSD   | X11 MIT-SHM extension                                  |
| 30  | shmat                  | Unimplemented | M8        | FreeBSD   |                                                        |
| 31  | shmctl                 | Unimplemented | M8        | FreeBSD   |                                                        |
| 32  | dup                    | Full          | M1        |           | Shell redirections                                     |
| 33  | dup2                   | Full          | M1        |           |                                                        |
| 34  | pause                  | Full          | M3        | Own       | Yields until signal delivered                          |
| 35  | nanosleep              | Full          | M1        | FreeBSD   | Parses timespec, delegates to _sleep_ms       |
| 36  | getitimer              | Unimplemented | M3        | FreeBSD   |                                                        |
| 37  | alarm                  | Stub          | M3        | Own       | Returns 0; no timer delivery yet                       |
| 38  | setitimer              | Unimplemented | M3        | FreeBSD   |                                                        |
| 39  | getpid                 | Full          | M1        |           |                                                        |
| 40  | sendfile               | Unimplemented | M4        | FreeBSD   | Efficient file-to-socket transfer                      |
| 41  | socket                 | Partial       | M1        |           | AF_INET, AF_UNIX; needs AF_NETLINK for systemd         |
| 42  | connect                | Full          | M1        |           |                                                        |
| 43  | accept                 | Full          | M1        |           |                                                        |
| 44  | sendto                 | Full          | M1        |           |                                                        |
| 45  | recvfrom               | Full          | M1        |           |                                                        |
| 46  | sendmsg                | Unimplemented | M4        | FreeBSD   | Needed for SCM_RIGHTS fd passing (D-Bus, systemd)      |
| 47  | recvmsg                | Unimplemented | M4        | FreeBSD   | Needed for SCM_RIGHTS fd passing                       |
| 48  | shutdown               | Partial       | M1        |           | how parameter ignored                                  |
| 49  | bind                   | Full          | M1        |           |                                                        |
| 50  | listen                 | Full          | M1        |           |                                                        |
| 51  | getsockname            | Full          | M1        |           |                                                        |
| 52  | getpeername            | Full          | M1        |           |                                                        |
| 53  | socketpair             | Unimplemented | M4        | FreeBSD   | FreeBSD has full AF_UNIX socketpair                     |
| 54  | setsockopt             | Unimplemented | M4        | FreeBSD   | SO_REUSEADDR, TCP_NODELAY, etc.                        |
| 55  | getsockopt             | Partial       | M1        |           | SOL_SOCKET: SO_ERROR, SO_TYPE                          |
| 56  | clone                  | Partial       | M3        | Own       | Fork-like clones work; CLONE_VM/CLONE_THREAD ENOSYS    |
| 57  | fork                   | Full          | M1        |           |                                                        |
| 58  | vfork                  | Full          | M1        |           | Implemented as fork() (safe, correct)                  |
| 59  | execve                 | Full          | M1        |           |                                                        |
| 60  | exit                   | Full          | M1        |           |                                                        |
| 61  | wait4                  | Full          | M1        |           | pid>0, pid==-1; correct (exit_code<<8) status encoding |
| 62  | kill                   | Full          | M1        |           |                                                        |
| 63  | uname                  | Full          | M1        |           | Reports "Linux 4.0.0 Kevlar"                           |
| 64  | semget                 | Unimplemented | M8        | FreeBSD   | SysV IPC for X11                                       |
| 65  | semop                  | Unimplemented | M8        | FreeBSD   |                                                        |
| 66  | semctl                 | Unimplemented | M8        | FreeBSD   |                                                        |
| 67  | shmdt                  | Unimplemented | M8        | FreeBSD   |                                                        |
| 68  | msgget                 | Unimplemented | M8        | FreeBSD   | SysV message queues                                    |
| 69  | msgsnd                 | Unimplemented | M8        | FreeBSD   |                                                        |
| 70  | msgrcv                 | Unimplemented | M8        | FreeBSD   |                                                        |
| 71  | msgctl                 | Unimplemented | M8        | FreeBSD   |                                                        |
| 72  | fcntl                  | Partial       | M1        | Own       | F_DUPFD, F_GETFD, F_SETFD, F_GETFL, F_SETFL, F_DUPFD_CLOEXEC |
| 73  | flock                  | Unimplemented | M3        | FreeBSD   | Advisory file locking                                  |
| 74  | fsync                  | Partial       | M1        |           | Delegates to opened_file                               |
| 75  | fdatasync              | Unimplemented | M3        | FreeBSD   |                                                        |
| 76  | truncate               | Unimplemented | M5        | FreeBSD   |                                                        |
| 77  | ftruncate              | Full          | M3        | Own       | Truncate file to specified length                      |
| 78  | getdents               | Unimplemented | M3        | FreeBSD   | Legacy readdir; some programs still use this            |
| 79  | getcwd                 | Full          | M1        |           |                                                        |
| 80  | chdir                  | Full          | M1        |           |                                                        |
| 81  | fchdir                 | Full          | M3        | Own       | Change directory by file descriptor                    |
| 82  | rename                 | Full          | M1        | FreeBSD   | Cross-dir rename with lock ordering           |
| 83  | mkdir                  | Full          | M1        |           |                                                        |
| 84  | rmdir                  | Full          | M1        | FreeBSD   | ENOTEMPTY check               |
| 85  | creat                  | Unimplemented | M5        | Own       | Legacy; usually via open(O_CREAT)                      |
| 86  | link                   | Full          | M1        |           |                                                        |
| 87  | unlink                 | Full          | M1        | FreeBSD   | EISDIR check                  |
| 88  | symlink                | Full          | M3        | Own       | ln -s command; tmpfs symlink support                   |
| 89  | readlink               | Full          | M1        |           | Includes partial /proc/self/fd                         |
| 90  | chmod                  | Full          | M1        |           |                                                        |
| 91  | fchmod                 | Stub          | M3        | Own       | Succeeds silently (tmpfs ignores permissions)          |
| 92  | chown                  | Stub          | M1        |           | Returns 0; no real UID tracking                        |
| 93  | fchown                 | Stub          | M3        | Own       | Succeeds silently                                      |
| 94  | lchown                 | Unimplemented | M3        | Own       |                                                        |
| 95  | umask                  | Full          | M1        |           | Per-process AtomicCell<u32>, propagated through fork   |
| 96  | gettimeofday           | Full          | M1        | FreeBSD   | Wall clock as timeval            |
| 97  | getrlimit              | Full          | M1        | FreeBSD   | Fake limits: 8MB stack, 1024 NOFILE|
| 98  | getrusage              | Stub          | M3        | Own       | Returns zeroed struct; real accounting deferred         |
| 99  | sysinfo                | Full          | M1        |           | Uptime, totalram/freeram from allocator, procs count   |
| 100 | times                  | Unimplemented | M3        | Own       |                                                        |
| 101 | ptrace                 | Unimplemented | M8        | FreeBSD   | Debuggers, strace                                      |
| 102 | getuid                 | Stub          | M1        |           | Returns 0                                              |
| 103 | syslog                 | Partial       | M1        |           | SYSLOG_ACTION_READ_ALL, SIZE_BUFFER                    |
| 104 | getgid                 | Stub          | M1        |           | Returns 0                                              |
| 105 | setuid                 | Stub          | M1        |           | Returns 0                                              |
| 106 | setgid                 | Stub          | M1        |           | Returns 0                                              |
| 107 | geteuid                | Stub          | M1        |           | Returns 0                                              |
| 108 | getegid                | Stub          | M1        |           | Returns 0 (no real GID tracking)                       |
| 109 | setpgid                | Full          | M1        |           |                                                        |
| 110 | getppid                | Full          | M1        |           |                                                        |
| 111 | getpgrp                | Full          | M1        |           | Reads process group ID                                 |
| 112 | setsid                 | Full          | M3        | Own       | Creates new session/process group                      |
| 113 | setreuid               | Unimplemented | M3        | Own       |                                                        |
| 114 | setregid               | Unimplemented | M3        | Own       |                                                        |
| 115 | getgroups              | Stub          | M3        | Own       | Returns 0 (no supplementary groups)                    |
| 116 | setgroups              | Stub          | M1        |           | Returns 0                                              |
| 117 | setresuid              | Unimplemented | M3        | Own       |                                                        |
| 118 | getresuid              | Unimplemented | M3        | Own       |                                                        |
| 119 | setresgid              | Unimplemented | M3        | Own       |                                                        |
| 120 | getresgid              | Unimplemented | M3        | Own       |                                                        |
| 121 | getpgid                | Full          | M1        | Own       | Supports both pid==0 and arbitrary pid                 |
| 122 | setfsuid               | Unimplemented | M5        | Own       |                                                        |
| 123 | setfsgid               | Unimplemented | M5        | Own       |                                                        |
| 124 | getsid                 | Partial       | M3        | Own       | Returns pgid as session ID (simplified)                |
| 125 | capget                 | Unimplemented | M4        | FreeBSD   | Linux capabilities                                     |
| 126 | capset                 | Unimplemented | M4        | FreeBSD   |                                                        |
| 127 | rt_sigpending          | Unimplemented | M3        | FreeBSD   |                                                        |
| 128 | rt_sigtimedwait        | Unimplemented | M3        | FreeBSD   |                                                        |
| 129 | rt_sigqueueinfo        | Unimplemented | M4        | Own       |                                                        |
| 130 | rt_sigsuspend          | Full          | M3        | Own       | Replaces mask, yields, restores, returns EINTR         |
| 131 | sigaltstack            | Stub          | M3        | Own       | Accepts and ignores; real alt stack deferred            |
| 132 | utime                  | Unimplemented | M5        | Own       | Legacy; utimensat preferred                             |
| 133 | mknod                  | Unimplemented | M5        | FreeBSD   | Create device nodes                                    |
| 134 | uselib                 | Unimplemented | —         |           | Obsolete                                                |
| 135 | personality            | Unimplemented | M8        | Own       | Execution domain                                       |
| 136 | ustat                  | Unimplemented | —         |           | Obsolete                                                |
| 137 | statfs                 | Unimplemented | M3        | FreeBSD   | df command                                             |
| 138 | fstatfs                | Unimplemented | M3        | FreeBSD   |                                                        |
| 139 | sysfs                  | Unimplemented | —         |           | Obsolete                                                |
| 140 | getpriority            | Unimplemented | M8        | FreeBSD   |                                                        |
| 141 | setpriority            | Unimplemented | M8        | FreeBSD   |                                                        |
| 142 | sched_setparam         | Unimplemented | M8        | FreeBSD   |                                                        |
| 143 | sched_getparam         | Unimplemented | M8        | FreeBSD   |                                                        |
| 144 | sched_setscheduler     | Unimplemented | M8        | FreeBSD   |                                                        |
| 145 | sched_getscheduler     | Unimplemented | M8        | FreeBSD   |                                                        |
| 146 | sched_get_priority_max | Unimplemented | M8        | FreeBSD   |                                                        |
| 147 | sched_get_priority_min | Unimplemented | M8        | FreeBSD   |                                                        |
| 148 | sched_rr_get_interval  | Unimplemented | M8        | FreeBSD   |                                                        |
| 149 | mlock                  | Unimplemented | M8        | FreeBSD   |                                                        |
| 150 | munlock                | Unimplemented | M8        | FreeBSD   |                                                        |
| 151 | mlockall               | Unimplemented | M8        | Own       |                                                        |
| 152 | munlockall             | Unimplemented | M8        | Own       |                                                        |
| 153 | vhangup                | Unimplemented | M4        | Own       |                                                        |
| 154 | modify_ldt             | Unimplemented | —         |           | Legacy x86                                              |
| 155 | pivot_root             | Unimplemented | M4        | Own       | systemd, containers                                    |
| 156 | sysctl                 | Unimplemented | —         |           | Deprecated; use /proc/sys                               |
| 157 | prctl                  | Unimplemented | M4        | FreeBSD   | PR_SET_NAME, PR_SET_CHILD_SUBREAPER, etc.              |
| 158 | arch_prctl             | Partial       | M1        |           | ARCH_SET_FS for TLS                                    |
| 159 | adjtimex               | Unimplemented | M8        | Own       | NTP                                                    |
| 160 | setrlimit              | Unimplemented | M3        | Own       | Use prlimit64(302) instead                              |
| 161 | chroot                 | Unimplemented | M4        | Own       |                                                        |
| 162 | sync                   | Unimplemented | M5        | Own       |                                                        |
| 163 | acct                   | Unimplemented | —         |           | Process accounting; rarely needed                       |
| 164 | settimeofday           | Unimplemented | M8        | Own       |                                                        |
| 165 | mount                  | Unimplemented | M4        | FreeBSD   | proc, sys, tmpfs, devfs                                 |
| 166 | umount2                | Unimplemented | M4        | FreeBSD   |                                                        |
| 167 | swapon                 | Unimplemented | —         |           |                                                        |
| 168 | swapoff                | Unimplemented | —         |           |                                                        |
| 169 | reboot                 | Partial       | M1        |           | Halts regardless of parameters                         |
| 170 | sethostname            | Unimplemented | M4        | FreeBSD   |                                                        |
| 171 | setdomainname          | Unimplemented | M4        | Own       |                                                        |
| 172 | iopl                   | Unimplemented | —         |           | Legacy x86 I/O                                          |
| 173 | ioperm                 | Unimplemented | —         |           | Legacy x86 I/O                                          |
| 174 | create_module          | Unimplemented | —         |           | Removed in Linux 2.6                                    |
| 175 | init_module            | Unimplemented | —         |           | No kernel module support                                |
| 176 | delete_module          | Unimplemented | —         |           |                                                        |
| 177 | get_kernel_syms        | Unimplemented | —         |           | Removed in Linux 2.6                                    |
| 178 | query_module           | Unimplemented | —         |           | Removed in Linux 2.6                                    |
| 179 | quotactl               | Unimplemented | —         |           |                                                        |
| 180 | nfsservctl             | Unimplemented | —         |           | Removed in Linux 3.1                                    |
| 181 | getpmsg                | Unimplemented | —         |           | Unimplemented in Linux                                  |
| 182 | putpmsg                | Unimplemented | —         |           | Unimplemented in Linux                                  |
| 183 | afs_syscall            | Unimplemented | —         |           | Unimplemented in Linux                                  |
| 184 | tuxcall                | Unimplemented | —         |           | Unimplemented in Linux                                  |
| 185 | security               | Unimplemented | —         |           | Unimplemented in Linux                                  |
| 186 | gettid                 | Partial       | M1        |           | Single-threaded; returns PID                           |
| 187 | readahead              | Unimplemented | M8        | Own       |                                                        |
| 188 | setxattr               | Unimplemented | M5        | Own       |                                                        |
| 189 | lsetxattr              | Unimplemented | M5        | Own       |                                                        |
| 190 | fsetxattr              | Unimplemented | M5        | Own       |                                                        |
| 191 | getxattr               | Unimplemented | M5        | Own       |                                                        |
| 192 | lgetxattr              | Unimplemented | M5        | Own       |                                                        |
| 193 | fgetxattr              | Unimplemented | M5        | Own       |                                                        |
| 194 | listxattr              | Unimplemented | M5        | Own       |                                                        |
| 195 | llistxattr             | Unimplemented | M5        | Own       |                                                        |
| 196 | flistxattr             | Unimplemented | M5        | Own       |                                                        |
| 197 | removexattr            | Unimplemented | M5        | Own       |                                                        |
| 198 | lremovexattr           | Unimplemented | M5        | Own       |                                                        |
| 199 | fremovexattr           | Unimplemented | M5        | Own       |                                                        |
| 200 | tkill                  | Unimplemented | M4        | Own       | Send signal to thread                                  |
| 201 | time                   | Unimplemented | M3        | FreeBSD   | Trivial wrapper around clock_gettime                    |
| 202 | futex                  | Partial       | M2        | FreeBSD   | WAIT/WAKE with per-address wait queues                 |
| 203 | sched_setaffinity      | Unimplemented | M8        | FreeBSD   |                                                        |
| 204 | sched_getaffinity      | Unimplemented | M8        | FreeBSD   |                                                        |
| 205 | set_thread_area        | Unimplemented | —         |           | x86-32 only                                             |
| 206 | io_setup               | Unimplemented | M8        | Own       | AIO                                                    |
| 207 | io_destroy             | Unimplemented | M8        | Own       |                                                        |
| 208 | io_getevents           | Unimplemented | M8        | Own       |                                                        |
| 209 | io_submit              | Unimplemented | M8        | Own       |                                                        |
| 210 | io_cancel              | Unimplemented | M8        | Own       |                                                        |
| 211 | get_thread_area        | Unimplemented | —         |           | x86-32 only                                             |
| 212 | lookup_dcookie         | Unimplemented | —         |           | OProfile; obsolete                                      |
| 213 | epoll_create           | Unimplemented | M4        | FreeBSD   | Legacy; use epoll_create1(291)                          |
| 214 | epoll_ctl_old          | Unimplemented | —         |           | Removed                                                 |
| 215 | epoll_wait_old         | Unimplemented | —         |           | Removed                                                 |
| 216 | remap_file_pages       | Unimplemented | —         |           | Deprecated since Linux 3.16                             |
| 217 | getdents64             | Partial       | M1        |           |                                                        |
| 218 | set_tid_address        | Partial       | M2        |           | Returns caller's TID; clear_child_tid not yet used     |
| 219 | restart_syscall        | Unimplemented | M3        | Own       | Signal restart mechanism                                |
| 220 | semtimedop             | Unimplemented | M8        | Own       |                                                        |
| 221 | fadvise64              | Unimplemented | M8        | FreeBSD   |                                                        |
| 222 | timer_create           | Unimplemented | M4        | Own       | POSIX timers                                            |
| 223 | timer_settime          | Unimplemented | M4        | Own       |                                                        |
| 224 | timer_gettime          | Unimplemented | M4        | Own       |                                                        |
| 225 | timer_getoverrun       | Unimplemented | M4        | Own       |                                                        |
| 226 | timer_delete           | Unimplemented | M4        | Own       |                                                        |
| 227 | clock_settime          | Unimplemented | M8        | Own       |                                                        |
| 228 | clock_gettime          | Full          | M1        |           | CLOCK_REALTIME, CLOCK_MONOTONIC                        |
| 229 | clock_getres           | Unimplemented | M4        | FreeBSD   | Trivial                                                |
| 230 | clock_nanosleep        | Unimplemented | M4        | FreeBSD   | High-precision sleep                                   |
| 231 | exit_group             | Partial       | M1        |           | Other threads TODO                                     |
| 232 | epoll_wait             | Unimplemented | M4        | FreeBSD   |                                                        |
| 233 | epoll_ctl              | Unimplemented | M4        | FreeBSD   |                                                        |
| 234 | tgkill                 | Full          | M3        | Own       | Send signal to specific thread (= kill by tid)         |
| 235 | utimes                 | Stub          | M1        |           | Only checks file existence, doesn't modify timestamps  |
| 236 | vserver                | Unimplemented | —         |           | Unimplemented in Linux                                  |
| 237 | mbind                  | Unimplemented | M8        | Own       | NUMA                                                   |
| 238 | set_mempolicy          | Unimplemented | M8        | Own       |                                                        |
| 239 | get_mempolicy          | Unimplemented | M8        | Own       |                                                        |
| 240 | mq_open                | Unimplemented | M8        | Own       | POSIX message queues                                    |
| 241 | mq_unlink              | Unimplemented | M8        | Own       |                                                        |
| 242 | mq_timedsend           | Unimplemented | M8        | Own       |                                                        |
| 243 | mq_timedreceive        | Unimplemented | M8        | Own       |                                                        |
| 244 | mq_notify              | Unimplemented | M8        | Own       |                                                        |
| 245 | mq_getsetattr          | Unimplemented | M8        | Own       |                                                        |
| 246 | kexec_load             | Unimplemented | —         |           |                                                        |
| 247 | waitid                 | Unimplemented | M3        | FreeBSD   | Extended wait (coreutils)                               |
| 248 | add_key                | Unimplemented | —         |           | Keyring                                                 |
| 249 | request_key            | Unimplemented | —         |           |                                                        |
| 250 | keyctl                 | Unimplemented | —         |           |                                                        |
| 251 | ioprio_set             | Unimplemented | M8        | Own       |                                                        |
| 252 | ioprio_get             | Unimplemented | M8        | Own       |                                                        |
| 253 | inotify_init           | Unimplemented | M4        | FreeBSD   | Legacy; use inotify_init1(294)                          |
| 254 | inotify_add_watch      | Unimplemented | M4        | FreeBSD   | Filesystem event monitoring (systemd)                   |
| 255 | inotify_rm_watch       | Unimplemented | M4        | FreeBSD   |                                                        |
| 256 | migrate_pages          | Unimplemented | —         |           | NUMA                                                   |
| 257 | openat                 | Full          | M1        | FreeBSD   | **Critical:** musl/glibc use this instead of open(2). Supports AT_FDCWD, dirfd resolution |
| 258 | mkdirat                | Full          | M3        | Own       | dirfd-relative mkdir                                   |
| 259 | mknodat                | Unimplemented | M5        | FreeBSD   |                                                        |
| 260 | fchownat               | Stub          | M3        | Own       | Succeeds silently                                      |
| 261 | futimesat              | Unimplemented | —         |           | Legacy; use utimensat(280)                              |
| 262 | newfstatat             | Full          | M1        | FreeBSD   | **Critical:** musl/glibc use this instead of stat(2). Supports AT_FDCWD, AT_EMPTY_PATH, AT_SYMLINK_NOFOLLOW |
| 263 | unlinkat               | Full          | M3        | Own       | AT_REMOVEDIR flag for rmdir; dirfd-relative             |
| 264 | renameat               | Full          | M3        | Own       | dirfd-relative rename                                   |
| 265 | linkat                 | Full          | M1        |           |                                                        |
| 266 | symlinkat              | Full          | M3        | Own       | dirfd-relative symlink creation                         |
| 267 | readlinkat             | Full          | M3        | Own       | dirfd-relative readlink                                 |
| 268 | fchmodat               | Stub          | M3        | Own       | Succeeds silently (tmpfs ignores permissions)          |
| 269 | faccessat              | Partial       | M1        | Own       | Delegates to access; flags not fully handled            |
| 270 | pselect6               | Unimplemented | M3        | Own       | Select with signal mask                                 |
| 271 | ppoll                  | Partial       | M1        | Own       | Delegates to poll; signal mask ignored                  |
| 272 | unshare                | Unimplemented | M7        | FreeBSD   | Namespaces                                              |
| 273 | set_robust_list        | Stub          | M2        | Own       | Accepts and ignores; real robust futex deferred         |
| 274 | get_robust_list        | Unimplemented | M2        | Own       |                                                        |
| 275 | splice                 | Unimplemented | M5        | FreeBSD   | Efficient fd-to-fd data transfer                        |
| 276 | tee                    | Unimplemented | M5        | Own       |                                                        |
| 277 | sync_file_range        | Unimplemented | M5        | Own       |                                                        |
| 278 | vmsplice               | Unimplemented | M8        | Own       |                                                        |
| 279 | move_pages             | Unimplemented | —         |           | NUMA                                                   |
| 280 | utimensat              | Unimplemented | M5        | FreeBSD   | Set file timestamps with nanosecond precision           |
| 281 | epoll_pwait            | Unimplemented | M4        | FreeBSD   | Signal-safe epoll wait                                  |
| 282 | signalfd               | Unimplemented | M4        | FreeBSD   | Legacy; use signalfd4(289)                              |
| 283 | timerfd_create         | Unimplemented | M4        | FreeBSD   | Timer file descriptors                                  |
| 284 | eventfd                | Unimplemented | M4        | FreeBSD   | Legacy; use eventfd2(290)                               |
| 285 | fallocate              | Unimplemented | M5        | FreeBSD   | Preallocate disk space                                  |
| 286 | timerfd_settime        | Unimplemented | M4        | FreeBSD   |                                                        |
| 287 | timerfd_gettime        | Unimplemented | M4        | FreeBSD   |                                                        |
| 288 | accept4                | Unimplemented | M6        | FreeBSD   | Accept with SOCK_CLOEXEC flag                           |
| 289 | signalfd4              | Unimplemented | M4        | FreeBSD   | Receive signals via fd (systemd)                        |
| 290 | eventfd2               | Unimplemented | M4        | FreeBSD   | Event notification fd                                   |
| 291 | epoll_create1          | Unimplemented | M4        | FreeBSD   | Scalable I/O event notification                         |
| 292 | dup3                   | Full          | M1        | FreeBSD   | dup2 with O_CLOEXEC. Returns EINVAL if oldfd==newfd     |
| 293 | pipe2                  | Full          | M1        | FreeBSD   | pipe with O_CLOEXEC, O_NONBLOCK flags                   |
| 294 | inotify_init1          | Unimplemented | M4        | FreeBSD   | Filesystem event monitoring                             |
| 295 | preadv                 | Unimplemented | M5        | Own       |                                                        |
| 296 | pwritev                | Unimplemented | M5        | Own       |                                                        |
| 297 | rt_tgsigqueueinfo      | Unimplemented | M4        | Own       |                                                        |
| 298 | perf_event_open        | Unimplemented | M8        | Own       |                                                        |
| 299 | recvmmsg               | Unimplemented | M6        | Own       |                                                        |
| 300 | fanotify_init          | Unimplemented | M8        | Own       |                                                        |
| 301 | fanotify_mark          | Unimplemented | M8        | Own       |                                                        |
| 302 | prlimit64              | Unimplemented | M2        | FreeBSD   | Get/set resource limits                                 |
| 303 | name_to_handle_at      | Unimplemented | M4        | Own       | File handle operations (systemd)                        |
| 304 | open_by_handle_at      | Unimplemented | M4        | Own       |                                                        |
| 305 | clock_adjtime          | Unimplemented | M8        | Own       |                                                        |
| 306 | syncfs                 | Unimplemented | M5        | Own       |                                                        |
| 307 | sendmmsg               | Unimplemented | M6        | Own       |                                                        |
| 308 | setns                  | Unimplemented | M7        | FreeBSD   | Join namespace                                          |
| 309 | getcpu                 | Unimplemented | M4        | FreeBSD   | Trivial                                                |
| 310 | process_vm_readv       | Unimplemented | M7        | Own       |                                                        |
| 311 | process_vm_writev      | Unimplemented | M7        | Own       |                                                        |
| 312 | kcmp                   | Unimplemented | M7        | Own       |                                                        |
| 313 | finit_module           | Unimplemented | —         |           | No kernel module support                                |
| 314 | sched_setattr          | Unimplemented | M8        | FreeBSD   |                                                        |
| 315 | sched_getattr          | Unimplemented | M8        | FreeBSD   |                                                        |
| 316 | renameat2              | Partial       | M3        | Own       | Delegates to renameat; RENAME_NOREPLACE flags TODO      |
| 317 | seccomp                | Unimplemented | M7        | FreeBSD   | Seccomp-bpf filters                                    |
| 318 | getrandom              | Full          | M1        |           |                                                        |
| 319 | memfd_create           | Unimplemented | M4        | Own       | Anonymous memory file (systemd)                         |
| 320 | kexec_file_load        | Unimplemented | —         |           |                                                        |
| 321 | bpf                    | Unimplemented | M7        | Own       | eBPF (containers, networking)                           |
| 322 | execveat               | Unimplemented | M5        | Own       | Execute by fd                                           |
| 323 | userfaultfd            | Unimplemented | M7        | Own       |                                                        |
| 324 | membarrier             | Unimplemented | M8        | Own       |                                                        |
| 325 | mlock2                 | Unimplemented | M8        | Own       |                                                        |
| 326 | copy_file_range        | Unimplemented | M5        | FreeBSD   |                                                        |
| 327 | preadv2                | Unimplemented | M5        | Own       |                                                        |
| 328 | pwritev2               | Unimplemented | M5        | Own       |                                                        |
| 329 | pkey_mprotect          | Unimplemented | M8        | Own       |                                                        |
| 330 | pkey_alloc             | Unimplemented | M8        | Own       |                                                        |
| 331 | pkey_free              | Unimplemented | M8        | Own       |                                                        |
| 332 | statx                  | Unimplemented | M5        | FreeBSD   | Extended stat (modern glibc)                            |
| 333 | io_pgetevents          | Unimplemented | M8        | Own       |                                                        |
| 334 | rseq                   | Unimplemented | M2        | Own       | Restartable sequences; can return ENOSYS initially      |
| 424 | pidfd_send_signal      | Unimplemented | M7        | Own       |                                                        |
| 425 | io_uring_setup         | Unimplemented | M8        | Own       |                                                        |
| 426 | io_uring_enter         | Unimplemented | M8        | Own       |                                                        |
| 427 | io_uring_register      | Unimplemented | M8        | Own       |                                                        |
| 428 | open_tree              | Unimplemented | M7        | Own       | New mount API                                           |
| 429 | move_mount             | Unimplemented | M7        | Own       |                                                        |
| 430 | fsopen                 | Unimplemented | M7        | Own       |                                                        |
| 431 | fsconfig               | Unimplemented | M7        | Own       |                                                        |
| 432 | fsmount                | Unimplemented | M7        | Own       |                                                        |
| 433 | fspick                 | Unimplemented | M7        | Own       |                                                        |
| 434 | pidfd_open             | Unimplemented | M7        | Own       |                                                        |
| 435 | clone3                 | Unimplemented | M7        | Own       | Modern clone with extensible struct                     |
