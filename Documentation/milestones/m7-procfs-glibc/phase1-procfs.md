# Phase 1: /proc VFS Skeleton

**Duration:** ~1 day
**Blocker for:** Phases 2-5
**Goal:** Mount /proc, implement directory structure and /proc/self symlink.

## Scope

Build the /proc VFS infrastructure.  After this phase, `/proc` is
mountable and browsable (ls /proc shows pid directories), but most
files are empty or missing.

## Implementation

### ProcFS service crate

Extend the existing procfs in `kernel/fs/procfs/` (not a separate
service crate — procfs needs direct access to process internals).

The current implementation already has:
- Basic mount/lookup infrastructure
- /proc/self/stat (partial)
- /proc/self/exe symlink

This phase cleans up the foundation:

### 1. Directory listing for /proc/

`readdir("/proc/")` must return:
- `self` — magic symlink to `/proc/[current_pid]/`
- One directory per live process: `1`, `2`, `3`, ...
- `cpuinfo`, `meminfo`, `version`, `mounts` (Phase 2 adds content)

Implementation: iterate the process table, emit one entry per PID.

### 2. /proc/self symlink

Already partially implemented.  Verify:
- `readlink("/proc/self")` returns the PID string (e.g., "1")
- `open("/proc/self/stat")` correctly resolves to `/proc/[pid]/stat`
- Works from any process, not just PID 1

### 3. /proc/[pid]/ directory

`readdir("/proc/42/")` must return:
- `stat`, `status`, `cmdline`, `maps`, `exe`, `fd`
- Permission: world-readable directory (0555) — Linux grants this

### 4. Inode numbering

Scheme: stable inodes derived from content, not allocation order:
- Root (/proc): ino = 1
- /proc/self: ino = 2
- /proc/cpuinfo: ino = 3
- /proc/meminfo: ino = 4
- /proc/version: ino = 5
- /proc/mounts: ino = 6
- /proc/[pid]/: ino = `(pid << 16) | 0`
- /proc/[pid]/stat: ino = `(pid << 16) | 1`
- /proc/[pid]/status: ino = `(pid << 16) | 2`
- etc.

## Testing

Contract test: `testing/contracts/subsystems/proc_mount.c`
```c
// Verify /proc is mountable and /proc/self resolves
// Verify readdir("/proc/") contains "self" and at least one PID dir
// Verify /proc/self/exe readlink works
```

## Success criteria

- [ ] `ls /proc/` shows `self`, PID directories, and file names
- [ ] `readlink /proc/self` returns current PID
- [ ] `/proc/[pid]/` directory exists for each live process
- [ ] M6.5 contract tests still pass
