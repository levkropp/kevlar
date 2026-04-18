# `strace-diff` — Kevlar vs Linux syscall comparison

Compare one Alpine binary's syscall sequence running on Kevlar versus running
natively on Linux against the same extracted Alpine rootfs. Highlights the
first point where Kevlar's contract diverges from Linux's.

The goal: triage XFCE userspace crashes (SIGSEGV, GP fault) by finding the
exact syscall where the two kernels return different things.

## One-time setup

Extract the Alpine rootfs so the Linux side can run the same musl binary that
Kevlar runs (no Docker needed, no sudo):

```
tools/extract-alpine-rootfs.py build/alpine-xfce.img build/alpine-xfce-rootfs
```

Takes ~15 seconds for 512 MB. Uses `debugfs rdump` under the hood.

## Running a diff

```
tools/strace-diff.py --linux-rootfs build/alpine-xfce-rootfs -- /bin/ls /etc
```

### What happens

1. **Linux reference**: `strace -f bwrap --bind alpine-xfce-rootfs / -- /bin/ls /etc`
   — runs the same Alpine musl binary in an unprivileged namespace, captures
   every syscall its trace.
2. **Kevlar**: boots a fresh Kevlar with `strace-pid=1 strace-exec=/bin/ls,/etc`
   on the cmdline. The `strace-target` PID-1 wrapper mounts the rootfs, chroots,
   and `execve`s the target. Kevlar's dispatch path emits one `DBG {...}` JSONL
   line per syscall.
3. **Parse + align**: both traces are trimmed to the last successful `execve`
   (that's where the target binary actually starts) and then aligned by call
   sequence.
4. **Diff**: for each aligned pair, checks syscall name, return value, and
   errno. First N mismatches are printed with context.

## Reading the output

```
#   6  name=open
       name:  linux=open  kevlar=mprotect
       linux args: "/lib/libXext.so.6", O_RDONLY|O_LARGEFILE|O_CLOEXEC
       kevlar args: [0xa00168000, 0x1000, 0x1, 0x0, 0xa0016b920, 0xa4d98]
```

At aligned call #6, Linux is calling `open("/lib/libXext.so.6", ...)` to start
loading libraries, but Kevlar skipped ahead and is doing `mprotect` on some
address — meaning Kevlar's musl loader took a different path earlier.

### Common noise

- Different return values for `set_tid_address`, `getpid`, `brk`, `mmap`:
  these return the PID or a memory address that naturally varies between runs.
  Treat as informational unless the call itself is missing.
- Different `getuid`/`getgid` values: Linux uses host UID, Kevlar uses 0 in
  chroot. Not a bug.

### Meaningful signals

- **Different syscall name** at the same position: Kevlar skipped or added
  a call vs Linux.
- **Linux returns success, Kevlar returns errno**: a contract gap.
- **Linux returns a specific value, Kevlar returns something different**:
  possible ABI mismatch.
- **Trace length differences**: Kevlar exiting earlier or looping more than
  Linux.

## CLI reference

```
tools/strace-diff.py [options] -- CMD [ARGS...]

--linux-rootfs PATH     extracted Alpine rootfs for bwrap (recommended)
--linux-trace FILE      skip running on Linux; parse a pre-recorded trace
--kevlar-only           run only the Kevlar side, dump JSON
--linux-only            run only the Linux side, dump JSON
--disk PATH             Kevlar disk image (default: build/alpine-xfce.img)
--init PATH             PID-1 on Kevlar (default: /bin/strace-target)
--max-diffs N           show at most N divergences (default: 20)
--timeout S             per-side timeout in seconds (default: 120)
--smp N                 Kevlar CPU count (default: 2)
--profile P             kernel profile (default: balanced)
--out-dir DIR           where to write traces (default: build/strace-diff)
```

## Adding a new target binary

Your command must be callable without spaces in arguments because the Kevlar
wrapper reads `strace-exec=` from `/proc/cmdline` and splits on `,`:

```
tools/strace-diff.py --linux-rootfs build/alpine-xfce-rootfs -- \
    /usr/bin/xdpyinfo -version
```

Becomes `strace-exec=/usr/bin/xdpyinfo,-version` on the cmdline.

## Known limitations

- No multi-process trace alignment yet. PID 1 = the target; children are
  traced but not aligned against Linux's fork/exec child traces.
- No semantic arg normalization: Kevlar args are raw u64 decimals, Linux
  args are symbolic (`O_RDONLY`, `AT_FDCWD`, string literals). Fixing this
  would require a Linux flag decoder — useful, not critical for v1.
- Some Kevlar traces may overflow the trace-record path (reentrant debug
  guard drops events). This manifests as a shorter Kevlar trace than
  reality; serial-log size is the usual cause.
