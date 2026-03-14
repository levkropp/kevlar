# Phase 1: Syscall Gap Closure

**Duration:** 3-4 days
**Prerequisite:** M8 complete
**Goal:** Implement the 5 missing syscalls and fix mount flag handling so that every syscall systemd calls in its first 1000 instructions returns a valid result.

## Missing Syscalls

### 1. waitid (247 on x86_64, 95 on ARM64) — CRITICAL

systemd uses `waitid(P_ALL, ...)` instead of `wait4(-1, ...)` for SIGCHLD handling in its main event loop.

**Implementation:** Reuse `wait4.rs` logic. Key differences:
- `idtype` parameter: `P_ALL=0`, `P_PID=1`, `P_PGID=2`
- Fills `siginfo_t` (128 bytes): `si_pid`, `si_uid`, `si_signo=SIGCHLD`, `si_status`, `si_code=CLD_EXITED/CLD_KILLED/CLD_STOPPED`
- `WNOWAIT` flag (0x01000000): peek without reaping
- Returns 0 on success (not pid like wait4)

**Files:** Create `kernel/syscalls/waitid.rs`, add to mod.rs dispatch.

### 2. memfd_create (319 on x86_64, 279 on ARM64)

Anonymous memory file. Create a tmpfs-backed file, return its fd.
Ignore `MFD_CLOEXEC`, `MFD_ALLOW_SEALING` flags (accept silently).

**Files:** Create `kernel/syscalls/memfd_create.rs`.

### 3. flock (73 on x86_64, 32 on ARM64)

Advisory file locking. Stub: always return 0. Real locking can be added later with per-inode lock state.

**Files:** Create `kernel/syscalls/flock.rs`.

### 4. close_range (436 on both)

Loop from `first` to `last`, close each fd. Handle `CLOSE_RANGE_CLOEXEC` (set O_CLOEXEC instead of closing).

**Files:** Create `kernel/syscalls/close_range.rs`.

### 5. pidfd_open (434 on both)

Return a pollable fd referring to a process. Becomes POLLIN-readable when the process exits.

**Files:** Create `kernel/syscalls/pidfd_open.rs`.

## Mount Flags

Extend `kernel/syscalls/mount.rs` and `kernel/fs/mount.rs`:

| Flag | Value | Behavior |
|------|-------|----------|
| MS_BIND | 0x1000 | Bind mount: source dir appears at target |
| MS_REMOUNT | 0x20 | Update flags on existing mount |
| MS_NOSUID | 0x2 | Store in MountEntry, check on execve |
| MS_NODEV | 0x4 | Store in MountEntry |
| MS_NOEXEC | 0x8 | Store in MountEntry, check on execve |

Add `flags: u32` field to `MountEntry`. Update `format_mounts()` and `format_mountinfo()` to include flag strings.

**MS_BIND implementation:** Look up source path as directory, mount that directory's filesystem at the target inode. Minimal: add source dir's inode as mount point at target.

## Contract Tests (6)

1. `waitid_basic.c` — fork, child exits, waitid with P_PID, verify siginfo_t fields
2. `memfd_basic.c` — memfd_create, write+read round-trip, ftruncate
3. `flock_basic.c` — open file, flock LOCK_EX, flock LOCK_UN
4. `close_range_basic.c` — open 5 fds, close_range(3, ~0U), verify closed
5. `pidfd_basic.c` — fork child, pidfd_open, poll, child exits, verify POLLIN
6. `mount_flags.c` — MS_BIND, MS_REMOUNT, verify /proc/mounts flags

## Success Criteria

- [ ] waitid returns correct siginfo_t for exited/killed/stopped children
- [ ] memfd_create returns writable anonymous fd
- [ ] flock returns 0 (stub)
- [ ] close_range closes fd range
- [ ] pidfd_open returns pollable process fd
- [ ] MS_BIND bind mounts work
- [ ] MS_REMOUNT updates mount flags
- [ ] All existing contract tests still pass
