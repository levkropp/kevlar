# M4 Phase 6: Integration Testing and Three Critical Bug Fixes

With all the individual M4 subsystems in place — epoll, signalfd, timerfd, eventfd,
Unix sockets, filesystem mounting, prctl, and capabilities — it was time to wire
them together and prove they actually work in concert. Writing `mini_systemd.c`
immediately uncovered three subtle bugs that had been lurking in the codebase.

## The Downcast Bug: Method Resolution vs. Trait Objects

The most insidious bug: `file.as_any().downcast_ref::<EpollInstance>()` always
returned `None`, even though `Debug` output showed `type=EpollInstance`. I spent
hours assuming this was TypeId instability with custom target specs.

The real cause was Rust method resolution. Given `file: &Arc<dyn FileLike>`:

```
file.as_any()
  → Arc<dyn FileLike>: Downcastable (blanket impl, since Arc is Sized+Any+Send+Sync)
  → returns &dyn Any wrapping Arc<dyn FileLike> itself
  → downcast_ref::<EpollInstance>() fails — inner type is Arc, not EpollInstance
```

The blanket `impl<T: Any + Send + Sync> Downcastable for T` applies to
`Arc<dyn FileLike>` because Arc is `Sized + 'static + Send + Sync`. Method
resolution finds this before auto-derefing through Arc to `dyn FileLike`.

The fix is explicit deref: `(**file).as_any()` dispatches through the
`dyn FileLike` vtable to the concrete type's `as_any()`, returning the actual
`EpollInstance` wrapped in `&dyn Any`.

This affected every `downcast_ref` call site in the codebase — epoll, timerfd,
and the existing sendmsg/recvmsg SCM_RIGHTS code (which had been silently failing).

## Signal Bitmask Off-by-One

`waitpid` was returning `EINTR` even though `SIGCHLD` was blocked via
`sigprocmask(SIG_BLOCK, ...)`. The cause: an off-by-one between internal and
userspace signal bitmask conventions.

- Internal `signal_pending`: `1 << signal` (SIGCHLD=17 → bit 17)
- Userspace `sigset_t`: `1 << (signal-1)` (SIGCHLD=17 → bit 16)

`has_pending_signals()` compared them directly: `pending & !blocked`. Bit 17
(pending SIGCHLD) was never masked by bit 16 (blocked SIGCHLD). Fix: align
internal representation to userspace convention using `1 << (signal - 1)`.

## socketpair and Timer Overflow

Two simpler fixes: implemented `socketpair(AF_UNIX, SOCK_STREAM)` by exposing
`UnixStream::new_pair()` (the building block already existed), and fixed a
`subtract with overflow` panic in `elapsed_msecs()` with `saturating_sub`.

## mini_systemd: 15 Tests, All Green

The integration test exercises the same codepaths as systemd PID 1 initialization:

| Test | What it exercises |
|------|-------------------|
| mount_proc, mount_meminfo, mount_mounts | /proc filesystem |
| prctl_name, prctl_subreaper | PR_SET_NAME, PR_SET_CHILD_SUBREAPER |
| capabilities | capget with v3 protocol |
| uid_gid | getuid/geteuid/getgid/getegid |
| epoll_create | epoll_create1(EPOLL_CLOEXEC) |
| signalfd | signalfd4 + epoll_ctl |
| timerfd | timerfd_create + timerfd_settime + epoll_ctl |
| eventfd | eventfd2 + write + epoll_ctl |
| unix_socket | socketpair + write + read |
| fork_exec | fork + _exit(42) + waitpid |
| epoll_eventfd, epoll_timerfd | Integrated epoll_wait loop |

All 15 tests pass under KVM. M4 is complete.
