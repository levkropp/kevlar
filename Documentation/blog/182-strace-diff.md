## Blog 182: strace-diff — an ABI-parity harness for Kevlar

**Date:** 2026-04-19

After [blog 181](181-first-graphics.md) proved Kevlar renders to a
framebuffer via Xorg and kxserver, the next question was: how do we
systematically find the remaining XFCE userspace crashes? The kernel
was stable, but XFCE components still SIGSEGV'd intermittently and we
had no principled way to find out which syscall contract we were
violating.

The answer: **run the same Alpine binary on both Kevlar and Linux,
capture every syscall, diff.** Every meaningful divergence is a
contract bug.

## The harness

`tools/strace-diff.py` — one command that boots a Kevlar VM with the
target as PID 1, runs the same binary on the host under strace against
the same Alpine rootfs (via `bwrap`), and produces a classified diff.

```
$ tools/strace-diff.py --linux-rootfs build/alpine-xfce-rootfs -- /usr/bin/xfwm4 --version

# Kevlar/Linux syscall diff — 880 aligned calls
  linux  trace : 881 calls  kevlar trace : 880 calls

## Classification
  MATCH             574  — Linux and Kevlar agreed
  NOISE (total)     306  — allowed variance (pointers/PIDs/UIDs/time)
    NOISE_POINTER      298
    NOISE_PID            3
    NOISE_UID            4
    NOISE_TIMING         1
  BUG   (total)       1  — real contract gap

# STRACE_DIFF BUGS bugs=1 match=574 noise=306 pairs=880 linux=881 kevlar=880
```

Kevlar side uses a PID-1 wrapper (`testing/strace_target.c`) that reads
`strace-exec=/path,arg1,arg2` from `/proc/cmdline`, mounts the virtio
disk, chroots, and execve's the target. The kernel emits one `DBG {...}`
JSONL line per syscall for the traced PID via
`DebugEvent::SyscallEntry/Exit`.

Linux side uses `bwrap` for an unprivileged chroot into an extracted
Alpine rootfs so we trace the *same* musl binary on both sides. No
Docker, no sudo.

## The classifier

A byte-exact match is impossible — pointers, PIDs, UIDs, and timing
all vary by design between kernels. But **semantic** equivalence is
the right bar, so divergences get categorized:

- `MATCH` — name, success/failure, errno, and return all agree.
- `NOISE_POINTER` — both sides returned a pointer from
  `mmap`/`brk`/`mremap`, different values are ASLR.
- `NOISE_PID` — both sides returned a positive TID/PID from
  `set_tid_address`/`getpid`/`fork`/`clone`.
- `NOISE_UID` — `getuid`/`getgid` family; host strace runs as your
  user, Kevlar runs as root in a fresh chroot.
- `NOISE_TIMING` — `clock_gettime`/`getrandom` or
  `write(fd=1|2, ...)` where byte counts differ due to embedded
  PIDs or timestamps in log text.
- `BUG_NAME` — different syscall at the same position (one side
  skipped or added a call).
- `BUG_ERRNO` — different failure mode.
- `BUG_RETVAL` — same outcome, different value, not in any noise
  category.

Only `BUG_*` counts toward a contract violation. The goal is simple:
`bugs=0` for every binary in the test set.

## Alignment

Naive index-based comparison breaks when one side does one fewer call
— from then on everything is misaligned and every entry looks like
a bug. The harness uses a **greedy look-ahead realign**: when names
diverge at position `i`, search forward up to 6 positions on each side
for a matching name, and report the skipped entries as unpaired. This
keeps divergence cascades bounded; a single skip produces one bug, not
hundreds.

Trimming is target-aware: `trim_to_target_execve(records, target_cmd)`
finds the execve whose args mention the requested binary path and
filters to that PID. Needed because `xfce4-session` forks `dbus-daemon`
and `strace -f` follows — without target-aware trimming, we'd end up
diffing dbus-daemon instead.

## What the harness found in one afternoon

**Bug 1 — `/proc/cmdline` hardcoded to `"kevlar\n"`**

The harness itself exposed this before it could run. `strace_target`
reads `strace-exec=` from `/proc/cmdline` to know what to run; every
call returned the same 7-byte string regardless of the actual kernel
cmdline. Any userspace tool reading kernel cmdline — systemd, our
harness, custom init scripts — would have failed silently.

Fixed by carrying `bootinfo.raw_cmdline: ArrayString<512>` through to
a `SpinLock<ArrayString>` that `ProcCmdlineFile::read` consults with
support for offset-based partial reads.

**Bug 2 — missing auxv entries**

`AT_EXECFN`, `AT_PLATFORM`, `AT_MINSIGSTKSZ`, `AT_HWCAP2`, and a zero
`AT_HWCAP` — all either missing or empty on Kevlar, always present
on Linux. glibc 2.34+ reads `AT_MINSIGSTKSZ`; musl's
`__progname_full` falls back to `AT_EXECFN`; libc code paths that
check `AT_HWCAP` for SSE/SSE2 see no features advertised.

Fixed by adding all four to the `Auxv` enum, auxv push/ptr
accounting, and supplying x86_64 platform + baseline HWCAP
(`FPU|MMX|SSE|SSE2`).

**Bug 3 — socket ops default errno**

`kevlar_vfs::inode::FileLike` defaults for `bind`, `listen`, `accept`,
`connect`, `shutdown`, `sendto`, `recvfrom`, `getsockname`,
`getpeername` all returned `EBADF`. Linux returns `ENOTSOCK` — the
fd is valid, it's just not a socket. The harness caught this with
xfwm4 calling `getpeername(fd=2, ...)` to probe whether stderr was a
socket.

Fixed by adding `ENOTSOCK = 88` to the `Errno` enum and replacing all
nine defaults.

## What's left for xfwm4 --version

**One bug:** libbrotlicommon's read-only rodata segment doesn't get
remapped on Kevlar where Linux does. Both kernels end up with the same
memory layout — the initial `PROT_READ` reserve already covers it —
and musl's internal code path chose to skip the redundant mmap. This
isn't a contract violation; the observable state is identical. But the
harness reports it, which is the right thing for a conservative test.

A more sophisticated classifier that recognizes "same-prot same-offset
MAP_FIXED no-op" as allowed optimization could retire this too.
Intentionally not doing so today: conservatism is better than
false-negative.

## The payoff shape

Before this work: the reliability test (`make test-xfce`) would
SIGSEGV one of xfce4-session / xfwm4 / xfdesktop in ~half of runs
with no way to tell *why*. After: any future regression surfaces as a
concrete syscall divergence with line numbers on both sides. The
bar for "ABI compatible" becomes a number that goes down with every
fix and that can't silently regress.

It's also the first time we have a testable statement of what "ABI
and kABI compatible with Linux" actually means for Kevlar — **MATCH
equal to pairs minus NOISE, every syscall accounted for, zero BUG**.
No more vibes-based claims of compatibility.

## Next targets

- `xfce4-session` produces 206 BUG_NAME entries right now, most of
  them alignment drift downstream of musl's library-load
  "already-loaded-by-name" fast path. Improving the classifier to
  recognize `open+fcntl+close` without intervening fstat as "cached
  library hit" will collapse the 206 down to the handful of real
  bugs.
- `xfdesktop` and `xfce4-panel` next.
- Then move up: `systemctl status`, `dbus-daemon`, init itself.
- Eventually: wire this into CI. One line:
  `# STRACE_DIFF BUGS bugs=1 match=574 noise=306 ...` is trivially
  grep-able. The regression test is: `bugs` must not increase.
