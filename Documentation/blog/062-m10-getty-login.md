# M10 Phase 2: Four Bugs Between Init and a Working Shell

BusyBox init processed `/etc/inittab`, ran all `::sysinit:` entries, spawned
getty — and then nothing. No output. No login prompt. Just silence on the
serial console for eternity. The fix required finding three independent bugs,
each in a different subsystem.

## Bug 1: POSIX fd allocation (the silent killer)

Getty's startup sequence does:
```
close(0)                           // close inherited stdin
open("/dev/ttyS0", O_RDWR)        // should get fd 0
dup2(0, 1); dup2(0, 2)            // copy stdin to stdout/stderr
```

The `open()` must return fd 0 (lowest available). POSIX requires this.
Our fd allocator used round-robin allocation starting from `prev_fd + 1`:

```rust
fn alloc_fd(&mut self, gte: Option<i32>) -> Result<Fd> {
    let (mut i, gte) = match gte {
        Some(gte) => (gte, gte),
        None => ((self.prev_fd + 1) % FD_MAX, 0),  // BUG
    };
    // ...
}
```

After several opens/closes, `prev_fd` pointed past 0, so `open()` returned
fd 3 instead of fd 0. Getty's `dup2(0, 1)` duplicated a closed fd.
Stdout/stderr ended up pointing to `/dev/null`. Getty wrote its login banner
to nowhere.

Fix: scan from 0, always return the lowest available fd.

```rust
fn alloc_fd(&mut self, gte: Option<i32>) -> Result<Fd> {
    let start = gte.unwrap_or(0);
    for i in start..FD_MAX {
        if matches!(self.files.get(i as usize), Some(None) | None) {
            return Ok(Fd::new(i));
        }
    }
    Err(Error::new(Errno::ENFILE))
}
```

## Bug 2: Missing TTY ioctls

With fd allocation fixed, getty progressed further but still produced no
output. Syscall tracing revealed getty calling several unhandled ioctls:

| ioctl | Name | Purpose |
|-------|------|---------|
| 0x5409 | TCSBRK | tcdrain — wait for output to drain |
| 0x540b | TCFLSH | tcflush — discard pending I/O |
| 0x5415 | TIOCMGET | Get modem control lines (carrier detect) |
| 0x5429 | TIOCGSID | Get session ID of terminal |

The original plan identified TIOCMGET as the root cause (getty checks carrier
detect without `-L`), but that was only part of the story. TCSBRK and TCFLSH
are called during termios setup; TIOCGSID during session validation.

All four are harmless to stub on a virtual serial port:
- TCSBRK: output is synchronous, nothing to drain
- TCFLSH: accept silently
- TIOCMGET: report carrier present + DSR
- TIOCGSID: return caller's PID as session ID

Also added TIOCMSET/TIOCMBIS/TIOCMBIC (modem control writes) as no-ops,
and the `-L` flag to the inittab getty line as defense in depth.

## Bug 3: Preemption permanently disabled (the deep one)

With ioctls and fds fixed, getty reached its termios setup, then called
`nanosleep(100ms)` — and never woke up. The 100ms timer expired, `resume()`
was called, PID 8 was set to Runnable and enqueued in the scheduler. But
nobody ever called `switch()` to actually run it.

The timer IRQ handler's preemption check:
```rust
if ticks % PREEMPT_PER_TICKS == 0 && !in_preempt() {
    return process::switch();
}
```

`in_preempt()` was **always true**. The per-CPU `preempt_count` was stuck
at a positive value, so the timer could never trigger a context switch.

### Root cause: leaked preempt_count in process entry points

`switch()` calls `preempt_disable()` before `do_switch_thread()`, and
`preempt_enable()` after it returns:

```rust
pub fn switch() -> bool {
    preempt_disable();           // preempt_count += 1
    // ... pick next process ...
    arch::switch_thread(prev, next);
    preempt_enable();            // preempt_count -= 1
    // ...
}
```

But newly created processes don't return through `switch()`. They enter via
assembly entry points that jump directly to userspace:

```asm
forked_child_entry:              // fork()'d children
    pop rdx                      // restore registers
    pop rdi
    // ...
    iretq                        // return to userspace
                                 // preempt_enable() never called!

userland_entry:                  // PID 1 (init)
    xor rax, rax                 // sanitize registers
    // ...
    iretq                        // return to userspace
                                 // preempt_enable() never called!
```

Every fork leaked +1 to `preempt_count`. PID 1 started with
preempt_count=1 (from its initial `switch()`). After 7 sysinit forks,
preempt_count was 8. Timer preemption was completely dead.

This bug was invisible during normal operation because processes
yield voluntarily via blocking syscalls (read, write, waitpid, exit all
call `switch()` internally). It only manifested when a process needed to
be woken by a timer — exactly what `nanosleep()` does.

Fix: decrement `preempt_count` at the top of both entry points:

```asm
forked_child_entry:
    mov eax, dword ptr gs:[GS_PREEMPT_COUNT]
    dec eax
    mov dword ptr gs:[GS_PREEMPT_COUNT], eax
    // ... rest of entry ...

userland_entry:
    mov eax, dword ptr gs:[GS_PREEMPT_COUNT]
    dec eax
    mov dword ptr gs:[GS_PREEMPT_COUNT], eax
    // ... rest of entry ...
```

Same fix applied to ARM64 (`mrs x0, tpidr_el1` + load/dec/store at
offset 16).

## Bug 4: TTY missing poll() (the post-login freeze)

After login, BusyBox sh displayed the `~ #` prompt and then froze.
No keyboard input was accepted. The shell was alive — it just never
read anything.

BusyBox sh with line editing uses `poll(fd, POLLIN, -1)` to wait for
input rather than blocking directly in `read()`. Our TTY had no `poll()`
implementation. The default returned `PollStatus::empty()` — "no events,
ever." The shell waited forever for poll to report data available.

Fix: implement `poll()` on the Tty to report POLLIN when the line
discipline buffer has data, and POLLOUT always (serial write is
synchronous):

```rust
fn poll(&self) -> Result<PollStatus> {
    let mut status = PollStatus::POLLOUT;
    if self.discipline.is_readable() {
        status |= PollStatus::POLLIN;
    }
    Ok(status)
}
```

## Debugging methodology

The investigation used progressive kernel-side tracing:

1. **TTY ioctl trace** — showed init processing sysinit but no tty
   activity from getty. Ruled out "getty never starts."

2. **Full syscall trace for PID 8** — showed getty opening `/dev/null`
   for stdout, revealing the fd allocation bug.

3. **fd-level trace** (open return values, dup2/close arguments) —
   confirmed `open("/dev/ttyS0")` returned fd 3 instead of fd 0.

4. **After fd fix**: getty progressed but ended in `nanosleep` with no
   write. Added nanosleep duration trace: 100ms sleep, never returned.

5. **Timer resume trace** — confirmed `resume()` was called, state
   changed to Runnable. But `switch()` never picked the process.

6. **Process state trace per tick** — revealed `in_preempt=true` on
   every timer tick. Led directly to the preempt_count leak.

Each layer peeled back one bug, revealing the next. Total: ~2 hours
from "no output" to "kevlar login:".

## Result

```
=== INIT READY ===

Kevlar (Alpine) kevlar /dev/ttyS0

kevlar login:
```

## Files changed

| File | Change |
|------|--------|
| `kernel/fs/opened_file.rs` | POSIX lowest-fd allocation |
| `kernel/fs/devfs/tty.rs` | TCSBRK, TCFLSH, TIOCMGET, TIOCGSID stubs + poll() impl |
| `platform/x64/usermode.S` | preempt_enable in userland_entry + forked_child_entry |
| `platform/arm64/usermode.S` | preempt_enable in userland_entry + forked_child_entry |
| `testing/etc/inittab` | `-L` flag on getty line |
