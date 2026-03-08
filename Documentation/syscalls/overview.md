# Syscall Coverage

Kevlar currently implements 59 Linux syscalls. The target is 170+ for full Linux userspace compatibility.

## Currently Implemented

| # | Name | Status |
|---|------|--------|
| 0 | read | Implemented |
| 1 | write | Implemented |
| 2 | open | Implemented |
| 3 | close | Implemented |
| 4 | stat | Implemented |
| 5 | fstat | Implemented |
| 6 | lstat | Implemented |
| 7 | poll | Implemented |
| 9 | mmap | Partial (prot ignored) |
| 12 | brk | Implemented |
| 13 | rt_sigaction | Partial |
| 14 | rt_sigprocmask | Implemented |
| 15 | rt_sigreturn | Implemented |
| 16 | ioctl | Partial |
| 20 | writev | Implemented |
| 22 | pipe | Implemented |
| 23 | select | Implemented |
| 33 | dup2 | Implemented |
| 39 | getpid | Implemented |
| 41 | socket | Implemented |
| 42 | connect | Implemented |
| 43 | accept | Implemented |
| 44 | sendto | Implemented |
| 45 | recvfrom | Implemented |
| 48 | shutdown | Implemented |
| 49 | bind | Implemented |
| 50 | listen | Implemented |
| 51 | getsockname | Implemented |
| 52 | getpeername | Implemented |
| 55 | getsockopt | Partial |
| 57 | fork | Implemented |
| 59 | execve | Implemented |
| 60 | exit | Implemented |
| 61 | wait4 | Implemented |
| 62 | kill | Implemented |
| 63 | uname | Implemented |
| 72 | fcntl | Partial |
| 74 | fsync | Stub |
| 79 | getcwd | Implemented |
| 80 | chdir | Implemented |
| 83 | mkdir | Implemented |
| 86 | link | Implemented |
| 89 | readlink | Implemented |
| 90 | chmod | Stub |
| 103 | syslog | Stub |
| 109 | setpgid | Implemented |
| 110 | getppid | Implemented |
| 121 | getpgid | Implemented |
| 158 | arch_prctl | Partial |
| 169 | reboot | Implemented |
| 186 | gettid | Implemented |
| 218 | set_tid_address | Implemented |
| 228 | clock_gettime | Implemented |
| 231 | exit_group | Implemented |
| 235 | utimes | Stub |
| 265 | linkat | Implemented |
| 318 | getrandom | Implemented |

## Missing (Priority)

See [Priority Syscalls](priority.md) for details.
