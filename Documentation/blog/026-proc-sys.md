# M5 Phase 4: /proc & /sys Completeness

Real-world programs don't just read files — they introspect the system
through /proc and /sys. Python checks /proc/self/maps, build systems read
/proc/cpuinfo, and every shell session polls stdin through fds that need
working poll() support. Phase 4 fills these gaps.

## Per-Process Enhancements

### /proc/[pid]/status — More Than Name and PID

The existing status file showed six fields. Programs like `ps`, `top`, and
crash handlers expect more. The enhanced version pulls data from multiple
kernel subsystems:

```
Name:   bench
State:  S (sleeping)
Tgid:   2
Pid:    2
PPid:   1
Uid:    0       0       0       0
Gid:    0       0       0       0
FDSize: 4
VmSize: 8320 kB
VmRSS:  8320 kB
Threads:        1
SigPnd: 0000000000000000
SigBlk: 0000000000000000
```

`FDSize` and open fd count come from `OpenedFileTable::table_size()` and
`count_open()` — two new methods added to the fd table. `VmSize` sums the
VMA lengths from the process's memory map. Signal masks read directly from
the process's `SignalDelivery` and `SigSet`.

### /proc/[pid]/maps — Memory Map

This is the file crash handlers, sanitizers, and JVM profilers read to
understand a process's virtual address space. Each VMA becomes one line:

```
7fbfffe000-7fc0000000 rw-p 00000000 00:00 0          [stack]
01001000-01001000 rw-p 00000000 00:00 0          [heap]
00200000-00204000 r-xp 00000000 00:00 0
```

The implementation iterates `Vm::vm_areas()`, formats permissions from
`MMapProt` flags (r/w/x + always 'p' for private), and labels the first
two anonymous VMAs as [stack] and [heap] (matching the kernel's VMA
creation order).

### /proc/[pid]/fd/ — File Descriptor Directory

Programs use this to enumerate open file descriptors — `ls /proc/self/fd/`
shows what a process has open. Each entry is a symlink to the file's path:

```
/proc/self/fd/0 -> /dev/console
/proc/self/fd/1 -> /dev/console
/proc/self/fd/2 -> /dev/console
```

The implementation is a virtual `Directory` that iterates the process's
`OpenedFileTable` using the new `iter_open()` method. Each open fd becomes
a symlink entry that resolves to `PathComponent::resolve_absolute_path()`.

## System-Wide Files

### /proc/cpuinfo

Build systems (GCC, CMake) and runtime feature detection (Python, JVM)
read cpuinfo to determine CPU capabilities. On x86_64, the implementation
reads the TSC calibration frequency for MHz and generates a standard Linux
cpuinfo block with vendor, model, flags, and bogomips. ARM64 gets a
MIDR-style block with implementer, architecture, and part number.

### /proc/uptime and /proc/loadavg

Simple system health files. Uptime reads from `read_monotonic_clock()` and
formats as seconds since boot. Loadavg reports 0.00 for all three averages
(accurate for our single-CPU workloads) with the current process count.

## Three Bugs, One Test Suite

Phase 4 exposed three latent kernel bugs that an automated test suite
would have caught immediately. So we built one.

### Bug 1: Default poll() Returns EBADF

The default `FileLike::poll()` implementation returned `Errno::EBADF`.
This meant poll() on any file that didn't override poll() — including the
TTY (stdin), all /proc files, and tmpfs regular files — would fail with
"bad file descriptor."

BusyBox's shell calls poll() on stdin during line editing. When poll()
returned EBADF, the shell treated it as a fatal error and exited.

Fix: change the default to return `PollStatus::POLLIN | PollStatus::POLLOUT`,
matching Linux behavior where regular files are always ready for I/O.

### Bug 2: SIGCHLD Interrupts Sleep Despite Ignore Disposition

When a child process exits, the parent gets SIGCHLD. Our `send_signal()`
unconditionally set the pending bit and woke the process. But SIGCHLD's
default disposition is "ignore" — it should NOT interrupt blocking syscalls.

The shell was sleeping in `read()` on stdin. SIGCHLD arrived (from cat
exiting), `sleep_signalable_until()` saw pending signals, returned EINTR,
and the shell exited with status 1.

Fix: `send_signal()` now checks the signal's current action. Signals with
`SigAction::Ignore` disposition are silently dropped — they're never
queued and never wake the process. Signals with explicit handlers or
terminate/stop/continue dispositions are delivered normally.

### Bug 3: sys_read Held fd Table Lock Across FileLike::read()

A performance optimization in `sys_read` held the opened file table's
spinlock for the entire duration of the read, avoiding a 20ns Arc
clone/drop. But this created a deadlock: reading `/proc/self/status`
calls `ProcPidStatus::read()` which locks the same fd table to count
open file descriptors (FDSize field).

Same issue in `sys_getdents64` — reading `/proc/self/fd/` tried to
enumerate the fd table while the directory fd's lock was still held.

Fix: both `sys_read` and `sys_getdents64` now clone the Arc<OpenedFile>
and release the fd table lock before calling into the file's read method.
The 20ns overhead is negligible compared to correctness.

### Mount Point Confusion (inode 0)

ProcPidDir returned inode number 0 from `stat()`. The mount table is
keyed by inode number, and if any mount point also had inode 0, the VFS
would incorrectly redirect `/proc/1/` to that filesystem. Fix: ProcPidDir
and ProcPidFdDir now return unique inode numbers (0x70000000 + pid).

## The Test Suite

These bugs motivated a dedicated syscall correctness test suite
(`tests/test.c`). It's a static musl binary that runs 24 tests covering:

- **Poll correctness** (5 tests): stdin, /dev/null, pipes, tmpfs, procfs
- **Procfs content** (8 tests): status, maps, fd/, cpuinfo, uptime, etc.
- **Basic syscalls** (11 tests): fork/wait, mmap, dup2, signals, etc.

`make test` builds the test binary, boots it as PID 1 in QEMU, and
checks for any FAIL lines. The test suite would have caught all three
bugs above on first run.

## What's Next

Phase 5 implements the VirtIO block device driver — the hardware
foundation for reading and writing disk sectors. This gives Kevlar access
to persistent storage for the first time, paving the way for ext2
filesystem support in Phase 6.
