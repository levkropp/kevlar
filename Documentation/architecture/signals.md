# Signal Handling

## Overview

Kevlar implements the full POSIX signal interface: `sigaction`, `sigprocmask`,
`sigpending`, `sigreturn`, `rt_sigaction`, `rt_sigprocmask`, `rt_sigreturn`,
`rt_sigpending`, `rt_sigtimedwait`, `sigaltstack`, `kill`, `tgkill`, `tkill`,
`rt_sigsuspend`, `pause`, and `signalfd`.

## Data Structures

### SigSet

`SigSet` is a compact `u64` newtype. Signal `n` maps to bit `n-1` (0-based, matching
the Linux `sigset_t` wire format):

```rust
pub struct SigSet(u64);

impl SigSet {
    pub fn is_blocked(self, sig: usize) -> bool {
        (self.0 & (1u64 << (sig - 1))) != 0
    }
}
```

The signal mask is stored as an `AtomicU64` on the process (`Process.sigset`), allowing
lock-free reads and writes with Relaxed ordering. `sigprocmask` achieves ~161 ns — 2x
faster than Linux KVM (~338 ns).

### SignalDelivery

Holds per-process signal state (shared across threads via `Arc<SpinLock<...>>`):

```rust
pub struct SignalDelivery {
    pending: u32,                       // Pending signals (0-based bitmask)
    actions: [SigAction; SIGMAX],       // Per-signal disposition
    nocldwait: bool,                    // Explicit sigaction(SIGCHLD, SIG_IGN)
}

pub enum SigAction {
    Ignore,
    Terminate,
    Stop,
    Continue,
    Handler { handler: UserVAddr, restorer: Option<UserVAddr> },
}
```

`Process.signal_pending` is an `AtomicU32` that mirrors `SignalDelivery.pending` for
a lock-free check on the hot path. This avoids taking the signal spinlock on every
syscall return when no signals are pending (the common case).

## Signal Delivery

After every syscall and on return from interrupt context, the kernel checks
`process.signal_pending` (lock-free). If non-zero:

```rust
pub fn try_delivering_signal(frame: &mut PtRegs) -> Result<()> {
    let current = current_process();
    // Fast path: no signals pending
    if current.signal_pending.load(Ordering::Relaxed) == 0 {
        return Ok(());
    }
    // Slow path: acquire lock, pop lowest unblocked signal
    let popped = {
        let mut sigs = current.signals.lock();
        let sigset = current.sigset_load();
        let result = sigs.pop_pending_unblocked(sigset);
        current.signal_pending.store(sigs.pending_bits(), Ordering::Relaxed);
        result
    };
    // Dispatch based on disposition...
}
```

Dispatch based on the signal's disposition:
- **SIG\_DFL** — run the default action (terminate, stop, ignore, or core dump)
- **SIG\_IGN** — discard the signal
- **Handler** — set up a signal frame on the user stack and jump to the handler

### Signal Frame (x86\_64)

For signals with a registered handler, the kernel:

1. Saves the current `PtRegs` into `signaled_frame` (for later restoration).
2. Subtracts 128 bytes from RSP (red zone avoidance).
3. Pushes a return address: either the `SA_RESTORER` trampoline (provided by musl/glibc)
   or an inline 8-byte trampoline that calls `rt_sigreturn`:

```asm
mov eax, 15        ; __NR_rt_sigreturn
syscall
nop
```

4. Sets `RIP = handler`, `RDI = signal number`, `RSI = 0`, `RDX = 0`.

`rt_sigreturn` restores the saved `PtRegs` to resume execution at the interrupted point.

### Signal Frame (ARM64)

Same approach but uses `x30` (LR) for the return address and `svc #0` with
`x8 = 139` for `rt_sigreturn`.

## SA\_SIGINFO

Handler functions registered with `SA_SIGINFO` receive three arguments:
`(signum: i32, info: *const siginfo_t, ctx: *const ucontext_t)`. Currently `siginfo`
and `ctx` are passed as null — full `siginfo_t` population is planned.

## Signal Reception

When a signal is sent to a process (`send_signal`):

```rust
pub fn send_signal(&self, signal: Signal) {
    // SIGCONT always continues a stopped process
    if signal == SIGCONT { self.continue_process(); }

    let mut sigs = self.signals.lock();
    // Signals with Ignore disposition are not queued
    if matches!(sigs.get_action(signal), SigAction::Ignore) { return; }
    sigs.signal(signal);
    drop(sigs);

    // Update lock-free mirror and wake the process
    self.signal_pending.fetch_or(1 << (signal - 1), Ordering::Release);
    self.resume();
}
```

## execve Behavior

On `execve`, all signal handlers are reset to `SIG_DFL` (old handler addresses are
invalid in the new address space). `SIG_IGN` dispositions are preserved. The signal
mask and pending set are preserved. The `nocldwait` flag is reset.

## signalfd

`signalfd` creates a file descriptor that can be read to consume blocked pending
signals. The implementation checks the process's pending signal set for signals
matching the signalfd's mask:

```rust
impl FileLike for SignalFd {
    fn read(&self, ...) -> Result<usize> {
        let mut sigs = current.signals().lock();
        while let Some(signal) = sigs.pop_pending_masked(self.mask) {
            writer.write_bytes(&make_siginfo(signal))?;
        }
        // Block if no signals and not O_NONBLOCK
        // ...
    }

    fn poll(&self) -> Result<PollStatus> {
        let pending = current_process().signal_pending_bits();
        if pending & self.mask != 0 { Ok(PollStatus::POLLIN) }
        else { Ok(PollStatus::empty()) }
    }
}
```

signalfd works with `epoll` for event-driven signal handling (used by systemd and
OpenRC).

## SIGSEGV Delivery

Userspace faults (null pointer, unmapped address, OOM during page fault) deliver
`SIGSEGV` with crash diagnostics:

1. Collect the last 32 syscalls from the per-process trace ring buffer.
2. Collect the VMA map and register state.
3. Emit a structured crash report as a JSONL debug event.
4. Exit with status `128 + SIGSEGV`.

## Default Actions

| Signal | Default Action |
|---|---|
| SIGTERM, SIGINT, SIGHUP, SIGPIPE, SIGALRM, SIGUSR1, SIGUSR2 | Terminate |
| SIGQUIT, SIGILL, SIGABRT, SIGFPE, SIGSEGV, SIGBUS | Terminate (core) |
| SIGCHLD, SIGURG, SIGWINCH | Ignore |
| SIGSTOP, SIGTSTP, SIGTTIN, SIGTTOU | Stop |
| SIGCONT | Continue if stopped |
| SIGKILL | Terminate (uncatchable) |
