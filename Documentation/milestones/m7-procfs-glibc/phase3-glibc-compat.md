# Phase 3: Per-Process /proc Files (stat, status, cmdline)

**Duration:** ~1.5 days
**Prerequisite:** Phase 1
**Goal:** Implement /proc/[pid]/stat, /proc/[pid]/status, /proc/[pid]/cmdline.

## Scope

These three files are the most-read per-process /proc files.  `ps`,
`top`, `htop`, systemd, and glibc all depend on them.

## Files

### 1. /proc/[pid]/stat

The existing implementation is partial.  Expand to full Linux format:

```
1 (init) S 0 1 1 0 -1 4194304 100 0 0 0 10 5 0 0 20 0 1 0 100 4096000 200 18446744073709551615 ...
```

52 space-separated fields.  Critical fields:
- Field 1: pid
- Field 2: (comm) — in parentheses
- Field 3: state (R/S/D/Z/T)
- Field 4: ppid
- Field 5: pgrp
- Field 7: tty_nr (0 for no terminal)
- Field 14: utime (clock ticks in user mode)
- Field 15: stime (clock ticks in kernel mode)
- Field 20: num_threads
- Field 22: starttime (boot-relative, in clock ticks)
- Field 23: vsize (virtual memory size in bytes)
- Field 24: rss (resident set size in pages)

Fields we can initially set to 0: minflt, cminflt, majflt, cmajflt,
nice, itrealvalue, signal, blocked, sigignore, sigcatch, wchan, nswap,
cnswap, exit_signal, processor, rt_priority, policy.

**Implementation:**
- Add `utime: AtomicU64` and `stime: AtomicU64` to Process struct
- Increment stime in syscall entry, utime in timer tick handler
- Add `start_time: u64` (set from TSC at process creation)
- VmSize from sum of VMA lengths, RSS from mapped page count

### 2. /proc/[pid]/status

Human-readable format (tools parse the field names):
```
Name:	init
Umask:	0022
State:	S (sleeping)
Tgid:	1
Pid:	1
PPid:	0
TracerPid:	0
Uid:	0	0	0	0
Gid:	0	0	0	0
FDSize:	64
VmPeak:	    4096 kB
VmSize:	    4096 kB
VmRSS:	     512 kB
Threads:	1
SigPnd:	0000000000000000
SigBlk:	0000000000000000
SigIgn:	0000000000000000
SigCgt:	0000000000000000
```

Tab-separated key-value pairs.  Fields:
- Name, State, Pid, PPid, Tgid: from Process struct
- Uid/Gid: from Process uid/gid (4 values: real, effective, saved, fs)
- VmSize: sum of VMA lengths / 1024
- VmRSS: count mapped pages * 4
- Threads: count threads in thread group
- Sig*: from SignalDelivery masks (hex, 16 digits)

### 3. /proc/[pid]/cmdline

Null-separated argv bytes:
```
/bin/sh\0-c\0echo hello\0
```

**Implementation:**
- Add `cmdline: Vec<u8>` to Process struct
- Populate during execve() — concatenate argv with null separators
- read() returns the raw bytes

## Process struct additions

```rust
// In Process:
pub cmdline: Vec<u8>,           // null-separated argv
pub start_time: u64,            // TSC at creation
pub utime: AtomicU64,           // user-mode ticks
pub stime: AtomicU64,           // kernel-mode ticks
```

Set `cmdline` in `Process::execve()`.  Set `start_time` in
`Process::new_init()` and `Process::fork()`.

## Testing

Contract test: `testing/contracts/subsystems/proc_pid.c`
```c
// Read /proc/self/stat, parse pid and state
// Read /proc/self/status, verify Name and Pid fields
// Read /proc/self/cmdline, verify argv[0] appears
```

## Success criteria

- [ ] `cat /proc/self/stat` shows 52-field format
- [ ] `cat /proc/self/status` shows Name, Pid, VmSize
- [ ] `cat /proc/self/cmdline` shows process name
- [ ] `ps` can parse /proc/[pid]/stat (basic output works)
