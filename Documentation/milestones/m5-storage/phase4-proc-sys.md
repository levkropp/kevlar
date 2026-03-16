# Phase 4: /proc & /sys Completeness

**Goal:** Fill the /proc and /sys gaps that real-world programs expect. Many
programs silently degrade or fail outright when specific /proc files are missing.

## New /proc Files

### Per-Process Files (/proc/[pid]/ and /proc/self/)

| File | Priority | Description |
|------|----------|-------------|
| `maps` | Required | Memory map — needed by crash handlers, sanitizers, Python, JVMs |
| `status` | Required | Process status (Name, State, Pid, Uid, VmRSS, Threads, etc.) |
| `fd/` | Required | Directory of open file descriptors (symlinks to targets) |
| `fdinfo/` | Nice-to-have | Per-fd details (pos, flags, mnt_id) |
| `cwd` | Nice-to-have | Symlink to current working directory |
| `exe` | Nice-to-have | Symlink to executable |
| `environ` | Nice-to-have | Environment variables (NUL-separated) |

### System-Wide Files

| File | Priority | Description |
|------|----------|-------------|
| `cpuinfo` | Required | CPU model, features, count — build systems, runtime detection |
| `stat` | Required | System statistics (cpu, processes, boot time) |
| `filesystems` | Required | Lists registered filesystem types |
| `uptime` | Nice-to-have | Seconds since boot |
| `loadavg` | Nice-to-have | Load averages (can report 0.00) |
| `version` | Nice-to-have | Kernel version string |
| `sys/kernel/osrelease` | Nice-to-have | Kernel release string (uname -r equivalent) |
| `sys/kernel/hostname` | Nice-to-have | System hostname |
| `sys/kernel/pid_max` | Nice-to-have | Max PID value |

## Design

### /proc/[pid]/maps

Format (one line per VMA):

```
address           perms offset  dev   inode  pathname
08048000-0804c000 r-xp 00000000 00:00 0      /bin/hello
0804c000-0804d000 rw-p 00004000 00:00 0      /bin/hello
b7e00000-b7f00000 rw-p 00000000 00:00 0      [heap]
bfffe000-c0000000 rw-p 00000000 00:00 0      [stack]
```

Implementation: iterate the process's VMA list (already maintained by the
memory manager). For each VMA, output start-end, permissions (rwxp), file
offset, device, inode number, and pathname.

Special names: `[heap]`, `[stack]`, `[vdso]`, or the executable path if the
VMA is file-backed.

### /proc/[pid]/status

Key-value format:

```
Name:   mini-systemd
State:  R (running)
Tgid:   1
Pid:    1
PPid:   0
Uid:    0       0       0       0
Gid:    0       0       0       0
FDSize: 8
VmPeak: 4096 kB
VmSize: 4096 kB
VmRSS:  1024 kB
Threads:        1
SigPnd: 0000000000000000
SigBlk: 0000000000000000
```

Implementation: pull from Process struct fields (pid, ppid, uid/gid, state,
comm, signal masks). VM stats from the process's page table / VMA list.

### /proc/[pid]/fd/

A directory where each entry is a symlink named by fd number, pointing to
the opened file's path:

```
/proc/self/fd/0 -> /dev/console
/proc/self/fd/1 -> /dev/console
/proc/self/fd/2 -> /dev/console
/proc/self/fd/3 -> anon_inode:[epoll]
```

Implementation: enumerate the process's `OpenedFileTable`, create a virtual
directory entry for each non-None slot. The symlink target is the
`PathComponent::resolve_absolute_path()` of the opened file.

### /proc/cpuinfo

```
processor       : 0
vendor_id       : GenuineIntel
model name      : QEMU Virtual CPU
cpu MHz         : 2400.000
cache size      : 0 KB
flags           : fpu vme de pse tsc msr ...
```

Implementation: read CPUID on x86_64 for vendor/model/features. On ARM64,
read MIDR_EL1 for implementer/part. CPU frequency from TSC calibration.
Hardcode reasonable values where detection is impractical.

### /proc/stat

```
cpu  10 0 5 1000 0 0 0 0 0 0
cpu0 10 0 5 1000 0 0 0 0 0 0
processes 42
procs_running 1
procs_blocked 0
btime 1709913600
```

Implementation: maintain global counters for context switches, process
creation, and boot time. Per-CPU time tracking deferred to M6 (SMP).

## /sys Stubs

Currently /sys is an empty mountpoint. Add minimal entries that programs
commonly check:

- `/sys/kernel/` — empty directory (existence check passes)
- `/sys/fs/` — empty directory
- `/sys/devices/` — empty directory

Real /sys population (device model, sysfs attributes) is deferred to when
we have a proper device model.

## Implementation Approach

All /proc files are implemented as `FileLike` objects in `kernel/fs/procfs/`.
The existing procfs infrastructure supports:
- `ProcDir` — virtual directory with named entries
- `ProcFile` — virtual file that generates content on `read()`

Add new files by creating new `ProcFile` implementations and registering
them in the procfs directory tree.

## Reference Sources

- Linux proc(5) man page — format specifications

## Testing

- `cat /proc/self/maps` shows VMAs with correct permissions and addresses
- `cat /proc/self/status` shows correct PID, PPID, name, state
- `ls /proc/self/fd/` lists open file descriptors
- `cat /proc/cpuinfo` shows processor information
- `cat /proc/stat` shows system statistics
- Programs that probe /proc (Python, GCC) don't crash on missing files
