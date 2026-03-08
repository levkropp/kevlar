# Priority Syscalls

These syscalls are critical for running complex Linux userspace software and are currently missing from Kevlar.

## Threading (Phase 1)

| # | Name | Purpose |
|---|------|---------|
| 56 | clone | Thread creation (CLONE_VM, CLONE_THREAD, etc.) |
| 202 | futex | Synchronization (mutexes, condition variables) |
| 234 | tgkill | Send signal to specific thread |
| 273 | set_robust_list | Mutex cleanup on thread exit |
| 274 | get_robust_list | Query robust list |

## Memory Management (Phase 2)

| # | Name | Purpose |
|---|------|---------|
| 10 | mprotect | Change page protections (needed for JIT, W^X) |
| 11 | munmap | Unmap memory regions |
| 25 | mremap | Resize/move mappings |
| 28 | madvise | Memory usage hints |

## Signals (Phase 3)

| # | Name | Purpose |
|---|------|---------|
| 131 | sigaltstack | Alternate signal stack for exception handling |
| 127 | rt_sigpending | Check pending signals |
| 128 | rt_sigsuspend | Wait for signal |
| 130 | rt_sigtimedwait | Timed signal wait |

## Events (Phase 4)

| # | Name | Purpose |
|---|------|---------|
| 232 | epoll_wait | Scalable I/O event notification |
| 233 | epoll_ctl | Manage epoll interest list |
| 281 | epoll_pwait | Signal-safe epoll wait |
| 290 | eventfd2 | Inter-thread notification |
| 283 | timerfd_create | Timer-based events |

## Filesystem (Phase 5)

Comprehensive `/proc` support is essential - many programs depend on `/proc/self/maps`
for address space introspection and `/proc/self/exe` for finding their own executable.

## Networking (Phase 6)

| # | Name | Purpose |
|---|------|---------|
| 46 | sendmsg | Control messages (SCM_RIGHTS) |
| 47 | recvmsg | Receive file descriptors |
| 53 | socketpair | Connected socket pair |
