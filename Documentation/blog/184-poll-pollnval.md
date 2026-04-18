## Blog 184: poll(2), invalid fds, and POLLNVAL

**Date:** 2026-04-19

With the kernel now stable (0/10 panics), the test-xfce score still
varied from 1/4 to 4/4 run-to-run. Looking at the SIGSEGV tally across
a 10-run sample:

```
5 cmd=/usr/bin/xfce4-session
3 cmd=Thunar
1 cmd=/usr/lib/tumbler-1/tumblerd
1 cmd=dbus-daemon
```

xfce4-session was half the crashes. The fault address — `0xe` or
`0x17` — is a dead giveaway: NULL-pointer-plus-small-offset, the
signature of `struct *p = NULL; p->field`.

The kernel's per-process syscall ring buffer (the last 32 syscalls
before a crash) gave us the rest of the story:

```
last 32 syscalls:
  nr=271 result=-9     a0=0x1e808c90 a1=0x2   ← ppoll returned -EBADF
  nr=0   result=8      a0=0x7                  ← read 8 bytes from fd=7
  nr=271 result=1      a0=0x1e808c90 a1=0x2   ← ppoll got 1 ready
  nr=1   result=8      a0=0x7                  ← write 8 bytes to fd=7
  nr=7   result=0      a0=0x1ec01f40 a1=0x1   ← poll, 0 ready
  nr=271 result=0      a0=0x1e808c90 a1=0x1   ← ppoll, 0 ready
  nr=47  result=56     a0=0x6                  ← recvmsg 56 bytes from fd=6
  nr=7   result=1      a0=0x1ec01f40 a1=0x1   ← poll, 1 ready
```

Right before the SIGSEGV, `ppoll` returned `-9` (`-EBADF`). On Linux
that's not supposed to happen for a mixed-validity pollfd array.

## The POSIX rule

[POSIX-2017 `poll(2)`](https://pubs.opengroup.org/onlinepubs/9699919799/functions/poll.html):

> The `revents` member shall be set by the implementation as follows:
> - `POLLNVAL`: the specified `fd` value is invalid.  This flag is
>   only valid in the `revents` bitmask; it shall be ignored in the
>   `events` member.

And on the `poll` function's return value:

> Upon successful completion, `poll()` shall return a non-negative
> value.  A value of 0 indicates that the call timed out and no file
> descriptors have been selected.  Upon failure, `poll()` shall
> return -1 and set errno to indicate the error.

The whole poll call does NOT fail when *one* fd is invalid.  That fd
just reports `POLLNVAL` in its `revents`, the other fds are processed
normally, and the call returns the count of fds with any events set
(including the POLLNVAL ones).

## What Kevlar did

`kernel/syscalls/poll.rs::sys_poll` before the fix:

```rust
let revents = if fd.as_int() < 0 || events.is_empty() {
    0
} else {
    let status = current_process().opened_files_no_irq().get(fd)?.poll()?;
    ...
};
```

The `?` on `opened_files_no_irq().get(fd)` propagates `EBADF` all the
way out of `sys_poll`, failing the syscall.  Every XFCE poll loop that
held a stale fd (e.g. a previously-open child dbus connection) hit this
on first poll, got `-EBADF` as the return value, and returned into
application code that wasn't ready for that failure mode.

xfce4-session's GLib main loop, for instance, treats `poll() < 0` as
a fatal error in some code paths, dereferences a NULL `GPollFD *`
in others, and SIGSEGVs with `fault_addr = NULL + offset` where the
offset is whichever field it tried to read.

## The fix

```rust
match current_process().opened_files_no_irq().get(fd) {
    Err(_) => {
        // Invalid fd — POSIX says mark it with POLLNVAL in revents
        // and continue. Do NOT fail the whole poll call.
        ready_fds += 1;
        PollStatus::POLLNVAL.bits()
    }
    Ok(file) => {
        let status = file.poll()?;
        let always = status & (PollStatus::POLLHUP | PollStatus::POLLERR);
        let revents = (events & status) | always;
        if !revents.is_empty() {
            ready_fds += 1;
        }
        revents.bits()
    }
}
```

`POLLNVAL` is bit 0x020 in Kevlar's `PollStatus` bitflags, same as
Linux. With this change, an invalid fd just appears as "ready" (with
the POLLNVAL flag set) in the output; the application reads `revents`,
sees `POLLNVAL`, handles the invalid fd locally (usually by closing
it in its poll set and retrying), and continues.

## Result

| metric | before | after |
|---|---|---|
| xfce4-session SIGSEGV rate | 5/10 | 3/10 |
| kernel panics | 0 | 0 |
| test-xfce scores | 1/4, 2/4×4, 3/4×2, (hung)×3, 4/4 | 1/4, 2/4, 3/4×4, 4/4×2, (hung)×2 |
| mean score (completed runs) | 2.4/4 | 3.0/4 |

The remaining 3 xfce4-session SIGSEGVs all hit the same user address
(`ip=0xa0006ac00, fault_addr=0x0, RDI=0x0`), so there is a second
NULL-deref bug in the same binary — but one that's triggered by a
different contract divergence than poll/PollNVAL.  Back to the
harness.

## The shape of these bugs

This is the fourth POSIX-contract divergence we've found since
[blog 182 documented the strace-diff harness](182-strace-diff.md):

1. `/proc/cmdline` hardcoded to `"kevlar\n"` (not a bug in syscall
   semantics, but in the data exposed through them)
2. auxv missing AT_EXECFN / AT_PLATFORM / AT_MINSIGSTKSZ / AT_HWCAP
3. socket ops default errno = EBADF instead of ENOTSOCK
4. **poll's failure model: EBADF from call instead of POLLNVAL in
   revents**

The pattern: Kevlar's implementation was usually "almost right" but
chose the convenient errno at an error site where POSIX specifies a
different mechanism.  Userspace compiled against Linux was written
around the POSIX mechanism; when Kevlar returned the convenient
errno, userspace's error handler wasn't the right one.

Each of these was one 15-line fix. And each was invisible to any
test that didn't exercise the specific error path. The strace-diff
harness exists precisely so we notice them without a SIGSEGV.
