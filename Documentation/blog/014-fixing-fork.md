# Fixing Fork: Two Bugs, One Wild Pointer

The fork benchmark was crashing with a page fault at `0x42c4ef` — an address
that didn't belong to any mapped region.  This looked like page table
corruption, register clobbering, or a bug in the context switch.  It turned
out to be neither.  Two missing POSIX semantics, interacting in a way that
only manifests when BusyBox `sh -c` is the init process, combined to produce
a deterministic wild jump.

## The symptom

Running `/bin/bench --quick fork` under KVM:

```
BENCH_START kevlar
BENCH_MODE quick
pid=1: no VMAs for address 000000000042c4ef (ip=42c4ef, reason=CAUSED_BY_USER | CAUSED_BY_INST_FETCH)
init exited with status 1, halting system
```

PID 1 is trying to *execute code* at `0x42c4ef`, but no VMA covers that
address.  The benchmark binary's text segment ends at `0x4069a1`.  Where
did `0x42c4ef` come from?

## Debugging strategy

Rather than reaching for GDB, I added targeted inline instrumentation:

1. **PtRegs corruption detection** in `dispatch()` — save `frame.rip` before
   the syscall, check it after `do_dispatch()` and again after
   `try_delivering_signal()`.  This pinpoints *which phase* corrupts the
   instruction pointer.

2. **`rt_sigaction` logging** — print the signal number, handler address,
   flags, and restorer for every `sigaction` call.

3. **VMA dump on fault** — when a page fault finds no matching VMA, dump all
   VMAs for the faulting process.

The results told the whole story in one boot:

```
rt_sigaction: signum=17, handler=0x42c4ef, flags=0x4000000, restorer=0x4428c5
...
SIGNAL DELIVERY: try_delivering_signal changed frame.rip from 0x4051dd to 0x42c4ef
pid=1: VMA dump (7 entries):
  VMA[3]: 0x401000-0x4069a1   ← this is bench's text, not BusyBox's
```

Signal 17 is SIGCHLD.  The handler at `0x42c4ef` is in BusyBox's text
segment (`0x401000-0x442a22`), not bench's (`0x401000-0x4069a1`).
PID 1's VMAs are bench's layout.

## Bug 1: `execve` didn't reset signal handlers

When `INIT_SCRIPT` is set, the kernel runs `/bin/sh -c "/bin/bench ..."`.
BusyBox sh registers a SIGCHLD handler at `0x42c4ef` during startup.  Many
shells optimize `sh -c "simple-command"` by exec'ing the command directly
without forking — so PID 1 does `execve("/bin/bench")`, replacing its
address space.

But Kevlar's `execve` never reset signal dispositions.  Per POSIX:

> Signals set to be caught by the calling process image shall be set to the
> default action in the new process image.

After exec, the handler function pointers from the old address space are
dangling.  Linux resets all `Handler { .. }` dispositions to `SIG_DFL` on
exec.  We weren't doing that.

**Fix:** Added `SignalDelivery::reset_on_exec()` — iterates the signal table
and resets any `Handler { .. }` entry to its POSIX default.  Called from
`Process::execve()`.

```rust
pub fn reset_on_exec(&mut self) {
    for i in 0..SIGMAX as usize {
        if matches!(self.actions[i], SigAction::Handler { .. }) {
            self.actions[i] = DEFAULT_ACTIONS[i];
        }
    }
}
```

## Bug 2: Default `Ignore` conflated with explicit `SIG_IGN`

With the first fix in place, fork no longer crashed — but it deadlocked.
The parent's `waitpid` would sleep forever.

The problem was in `Process::exit()`:

```rust
if parent.signals().lock().get_action(SIGCHLD) == SigAction::Ignore {
    // Auto-reap: remove child from parent's children list
    parent.children().retain(|p| p.pid() != current.pid);
    EXITED_PROCESSES.lock().push(current.clone());
} else {
    parent.send_signal(SIGCHLD);
}
```

Our `DEFAULT_ACTIONS` table has `SigAction::Ignore` for SIGCHLD (index 17).
After `reset_on_exec()` resets the SIGCHLD handler to default, `get_action`
returns `Ignore` — and the auto-reap code removes the zombie before
`waitpid` can find it.

But this conflates two different things:

- **Default disposition** (`SIG_DFL` for SIGCHLD): "don't kill the process
  on SIGCHLD" — but zombies are still created for `wait()`.
- **Explicit `SIG_IGN`** via `sigaction(SIGCHLD, {SIG_IGN})`: auto-reap,
  `wait()` returns `ECHILD`.

Linux only auto-reaps when SIGCHLD is explicitly set to `SIG_IGN` or when
`SA_NOCLDWAIT` is set.  The default disposition creates zombies normally.

**Fix:** Remove the auto-reap shortcut entirely.  Always create a zombie
and send SIGCHLD.  Proper `SA_NOCLDWAIT` / explicit `SIG_IGN` tracking is
a future task.

## The interaction

Neither bug alone was obvious:

- Bug 1 alone: the dangling handler pointer causes a crash, but only when
  `sh -c` exec-optimizes (which BusyBox does for simple commands).
- Bug 2 alone: harmless as long as signal handlers survive exec (the
  auto-reap path was only reached because bug 1's fix exposed it).
- Together: fix the crash, get a deadlock.  Fix the deadlock, fork works.

## Result

All 8 benchmarks now pass:

```
BENCH getpid     10000  134000000   13400
BENCH read_null   5000  137000000   27400
BENCH write_null  5000  143000000   28600
BENCH pipe          32   13000000  406250
BENCH fork_exit     50 4155000000 83100000
BENCH open_close  2000  203000000  101500
BENCH mmap_fault   256   48000000  187500
BENCH stat        5000 1336000000  267200
```

## Auto-reap: done right

With the root cause understood, implementing proper auto-reap was
straightforward:

1. Added `nocldwait: bool` to `SignalDelivery` — only set when the user
   explicitly calls `sigaction(SIGCHLD, SIG_IGN)`, never by the default
   disposition.
2. `rt_sigaction` sets `nocldwait` when SIGCHLD is explicitly set to
   `SIG_IGN`.
3. `Process::exit()` checks `parent.signals().lock().nocldwait()` — only
   auto-reaps when the flag is true.
4. `wait4` returns `ECHILD` when no matching children exist (prevents
   deadlock if all children were auto-reaped).
5. `reset_on_exec()` clears `nocldwait`.

## Lessons

- **Inline instrumentation beats GDB for kernel debugging** — adding three
  `debug_warn!` calls and one VMA dump identified the root cause in a
  single boot cycle.  No breakpoints, no stepping, no symbol loading.
- **POSIX compliance bugs compose** — two independently harmless deviations
  from the spec combined to produce a crash-then-deadlock sequence.
- **Know your init process** — `sh -c "cmd"` is not the same as running
  `cmd` directly.  The shell's exec optimization means PID 1 changes
  identity, and any state that survives exec (like signal handlers) is
  wrong if not properly cleaned up.
