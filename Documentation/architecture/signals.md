# Signal Handling

## Overview

Kevlar implements the full POSIX signal interface: `sigaction`, `sigprocmask`,
`sigpending`, `sigreturn`, `rt_sigaction`, `rt_sigprocmask`, `rt_sigreturn`,
`rt_sigpending`, `rt_sigtimedwait`, `sigaltstack`, `kill`, `tgkill`, `tkill`.

## Data Structures

### SigSet

`SigSet` is a compact `u64` newtype. Signal `n` maps to bit `n-1` (0-based, matching
the Linux `sigset_t` wire format). Standard bitwise operations (`BitOrAssign`,
`BitAndAssign`, `Not`) work on `SigSet` values directly.

The signal mask is stored as an `AtomicU64` in `SignalDelivery`, allowing lock-free
reads and writes (Relaxed ordering). `sigprocmask` achieves ~161 ns — faster than
Linux KVM (~338 ns).

### SignalDelivery

Holds per-process signal state:

| Field | Type | Purpose |
|---|---|---|
| `handlers` | `[SigAction; 64]` | Signal disposition (SIG_DFL, SIG_IGN, or handler address) |
| `mask` | `AtomicU64` | Blocked signal set |
| `pending` | `u64` | Pending signals (0-based bitmask) |
| `nocldwait` | `bool` | Set by explicit `sigaction(SIGCHLD, SIG_IGN)` |
| `altstack` | `Option<SigAltStack>` | Alternate signal stack from `sigaltstack(2)` |

`Process.signal_pending` is an `AtomicU32` that mirrors `SignalDelivery.pending` for
a lock-free check in the hot path before each syscall return.

## Signal Delivery

After every syscall and on return from interrupt context, the kernel checks
`process.signal_pending` (lock-free). If non-zero:

1. Lock `SignalDelivery`, find the lowest-numbered unblocked pending signal.
2. Clear the bit from `pending`.
3. Dispatch based on the disposition:
   - **SIG_DFL** — run the default action (terminate, stop, ignore, or core dump)
   - **SIG_IGN** — discard the signal
   - **Handler** — set up a signal frame on the user stack and return to the handler

### Signal Frame

For signals with a registered handler, the kernel pushes a `SignalFrame` onto the
user stack (or the alternate signal stack if `SA_ONSTACK` is set):

- Saved user registers (`ucontext_t`)
- `siginfo_t` (for `SA_SIGINFO` handlers)
- A trampoline that calls `rt_sigreturn`

`rt_sigreturn` restores the saved context to resume execution at the interrupted point.

## SA_SIGINFO

Handler functions registered with `SA_SIGINFO` receive three arguments:
`(signum: i32, info: *const siginfo_t, ctx: *const ucontext_t)`. The `siginfo_t`
is populated with the signal's code and relevant fields (e.g., `si_pid` for `kill(2)`,
`si_addr` for `SIGSEGV`).

## execve Behavior

On `execve`, all signal handlers are reset to `SIG_DFL`. The signal mask and `nocldwait`
flag are preserved across exec. This matches POSIX behavior and prevents child
processes from jumping to handler addresses that are no longer valid.

## Default Actions

| Signal | Default Action |
|---|---|
| SIGTERM, SIGINT, SIGHUP, SIGPIPE, SIGALRM, SIGUSR1, SIGUSR2 | Terminate |
| SIGQUIT, SIGILL, SIGABRT, SIGFPE, SIGSEGV, SIGBUS | Terminate (core) |
| SIGCHLD, SIGURG, SIGWINCH | Ignore |
| SIGSTOP, SIGTSTP, SIGTTIN, SIGTTOU | Stop |
| SIGCONT | Continue if stopped |
| SIGKILL | Terminate (uncatchable) |
