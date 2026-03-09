# Milestone 3: Terminal Control, Job Control, and the Road to Bash

**Date:** 2026-03-08

---

Kevlar now has the syscall infrastructure for running Bash and GNU Coreutils. M3 added terminal control, job control, symlinks, `*at` syscalls, proper signal defaults, and a `clone` implementation — bringing the kernel from 79 to 103 implemented syscalls (116 dispatch entries). The shell is more capable, and the groundwork is laid for running real interactive programs.

```
/ # echo hello
hello
/ # ls /
bin                integration_tests  sbin               usr
dev                lib                sys                var
etc                proc               tmp
/ # /bin/hello-dynamic
hello from dynamic linking!
```

BusyBox's ash shell boots and works interactively. Dynamic linking from M2 still works. The next step is adding Bash itself to the initramfs and testing against GNU Coreutils.

## What changed

M3 was less about dramatic new capabilities and more about filling in the dozens of small syscalls and fixes that real programs expect. Bash doesn't need any single exotic feature — it needs everything to be *slightly more correct*.

### Signal defaults were wrong

The inherited Kerla signal table had most signals defaulting to `Ignore`. Per POSIX signal(7), most signals default to `Terminate`. Only SIGCHLD, SIGCONT, SIGURG, and SIGWINCH default to Ignore. This meant programs relying on default SIGHUP, SIGPIPE, or SIGTERM behavior would silently ignore fatal signals instead of dying:

```rust
// Before (wrong): almost everything was Ignore
/* SIGHUP  */ SigAction::Ignore,
/* SIGPIPE */ SigAction::Ignore,
/* SIGTERM */ SigAction::Ignore,

// After (correct): per POSIX signal(7)
/* SIGHUP  */ SigAction::Terminate,
/* SIGPIPE */ SigAction::Terminate,
/* SIGTERM */ SigAction::Terminate,
```

### Terminal control: the Termios rewrite

BusyBox's shell and any real terminal program needs `TCGETS`/`TCSETS` to query and set terminal attributes, and `TIOCGWINSZ` to get window size. The existing Termios struct didn't match the Linux kernel ABI.

The fix was a complete rewrite of `struct Termios` to match Linux's `asm-generic/termbits.h`: four `u32` flag fields (`c_iflag`, `c_oflag`, `c_cflag`, `c_lflag`), one `u8` line discipline, and a 19-byte `c_cc` control character array — 36 bytes total, `#[repr(C)]`. The default `c_cc` sets ^C for VINTR, ^Z for VSUSP, ^D for VEOF, and DEL for VERASE.

TCGETS (0x5401), TCSETS (0x5402), TCSETSW, TCSETSF, TIOCGWINSZ (0x5413), and TIOCSWINSZ (0x5414) are now implemented on the serial TTY, PTY master, and PTY slave. Line discipline handles ^C (SIGINT), ^Z (SIGTSTP), and ^D (EOF) based on the ISIG and ICANON flags.

### Job control

Bash needs to stop and continue child processes. M3 added:

- **`ProcessState::Stopped(Signal)`** — a new process state alongside `Runnable`, `BlockedSignalable`, and `ExitedWith`
- **`SigAction::Stop` and `SigAction::Continue`** — SIGSTOP/SIGTSTP/SIGTTIN/SIGTTOU now stop processes instead of terminating them, and SIGCONT continues them
- **SIGCONT in `send_signal()`** — sending SIGCONT to a stopped process resumes it immediately, even if SIGCONT is blocked (per POSIX)
- **`wait4(WUNTRACED)`** — the parent can now collect stopped children with correct Linux wait status encoding: `(signo << 8) | 0x7f`

### Symlinks and *at syscalls

tmpfs gained a `TmpFsSymlink` type implementing the `Symlink` trait. The initramfs builder creates symlinks, and programs can create them at runtime with `symlink`/`symlinkat`. New `*at` syscalls provide dirfd-relative path resolution:

| Syscall | Purpose |
|---------|---------|
| `unlinkat` | Remove file or directory (with `AT_REMOVEDIR`) |
| `mkdirat` | Create directory relative to dirfd |
| `renameat`/`renameat2` | Rename relative to dirfd |
| `readlinkat` | Read symlink target relative to dirfd |
| `symlinkat` | Create symlink relative to dirfd |
| `fchdir` | Change directory by file descriptor |

### clone

musl's `fork()` calls `clone(SIGCHLD, 0, ...)`. Kevlar previously dispatched this directly to `sys_fork()` without looking at flags. The new `clone` implementation parses Linux clone flags and handles fork-like clones properly. Full threading (`CLONE_VM`/`CLONE_THREAD`) returns `ENOSYS` for now — that's M4 territory.

### Bug fixes that matter

- **O_EXCL:** the condition was inverted — `EEXIST` was suppressed when `O_EXCL` *was* set instead of when it *wasn't*
- **O_TRUNC:** opening a file with `O_TRUNC | O_WRONLY` now truncates it
- **O_APPEND:** writes now seek to EOF before writing
- **kill(pid < -1):** was signaling the current process group instead of process group `abs(pid)`
- **rt_sigaction oldact:** now writes the old handler value before overwriting with the new action
- **fcntl F_DUPFD:** was disabled (match arm was unreachable)
- **getpgid(non-zero pid):** was `todo!()`; now looks up the target process

### Additional stubs

Programs probe for syscalls and fall back gracefully when they get `ENOSYS`. But some syscalls are called unconditionally by libc internals, and returning `ENOSYS` causes crashes. M3 added stubs for:

- `tgkill` — signal by thread ID (behaves like `kill` until we have real threads)
- `rt_sigsuspend` — temporarily replace signal mask and wait
- `pause` — wait for signal
- `alarm` — returns 0 (no timer delivery yet)
- `fchmod`/`fchmodat`/`fchownat` — succeed silently (tmpfs ignores permissions)
- `getgroups` — returns 0 supplementary groups
- `getrusage` — returns zeroed struct
- `sigaltstack` — returns 0

### Warning cleanup

All compiler warnings from Rust 2024's `unsafe_op_in_unsafe_fn` lint and stale feature gates were fixed. `make check` now produces zero warnings on both x86_64 and ARM64.

## Provenance

Every M3 syscall was implemented from POSIX and Linux man pages. No BSD or other copyleft-licensed source code was referenced. The provenance log at `Documentation/provenance/clean-room-log.md` tracks each syscall individually — all are marked "Own."

This matters because Kevlar's value proposition is its triple license (MIT/Apache-2.0/BSD-2-Clause). We track provenance rigorously so that every syscall implementation can demonstrate clean-room origins.

## Syscall count

| Milestone | Target | Actual | Status |
|-----------|--------|--------|--------|
| M1: Static Busybox | ~50 | 79 | Complete |
| M1.5: ARM64 | -- | 79 | Complete |
| M2: Dynamic linking | ~55 | 83 | Complete |
| M3: Coreutils + Bash | ~80 | 103 | Complete |

## What's next

**M4: systemd (PID 1).** The next milestone targets running systemd as PID 1, which requires `epoll`, `signalfd`, `timerfd`, and `mount`. The job control and signal infrastructure from M3 carries forward — systemd relies heavily on correct signal handling and process lifecycle management.

Before M4, we'll add Bash to the initramfs and validate the M3 syscall coverage against a real Bash session. The kernel has the syscalls; now it needs the binaries.
