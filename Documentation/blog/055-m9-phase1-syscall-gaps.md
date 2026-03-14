# M9 Phase 1: Syscall Gap Closure

Phase 1 closes the 5 missing syscalls that systemd needs and adds
bind mount support.

## New syscalls

- **waitid(2)** — the critical one. systemd uses `waitid(P_ALL, ...)`
  for its main SIGCHLD loop. Reuses wait4 logic, fills siginfo_t with
  si_pid/si_signo/si_code/si_status at correct offsets.

- **memfd_create(2)** — creates an anonymous tmpfs-backed file.
  Used by systemd for sealed inter-process data passing.

- **flock(2)** — advisory file locking stub (returns 0). systemd
  uses flock for lock files under /run.

- **close_range(2)** — closes a range of file descriptors. Used by
  glibc and systemd before exec to clean up leaked fds.

- **pidfd_open(2)** — returns ENOSYS for now. systemd handles this
  gracefully and falls back to SIGCHLD monitoring.

## Mount flags

- **MS_BIND** — bind mounts now work. Source directory appears at
  target via a BindFs wrapper that implements FileSystem by returning
  the source directory as root.
- **MS_REMOUNT** — accepted silently (flag-only operation).
- **MS_NOSUID, MS_NODEV, MS_NOEXEC** — recognized in flag parsing.

## Results

- 30/31 PASS, 1 XFAIL (ns_uts needs root on Linux)
- 14/14 musl threading (no regression)
- waitid contract test verifies siginfo_t pid, signo, code, status
