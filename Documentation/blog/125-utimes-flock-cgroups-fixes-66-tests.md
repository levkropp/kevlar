# Blog 125: utimes, flock, cgroups PID leak fix, 66/66 Alpine tests pass

**Date:** 2026-03-28
**Milestone:** M10 Alpine Linux

## Summary

Four major improvements to Alpine Linux compatibility:

1. **Real file timestamps** -- `utimes`/`utimensat` now modify ext4 inode
   atime/mtime/ctime on disk
2. **Advisory file locking** -- `flock(2)` with per-OFD lock table, contention,
   and auto-release on close
3. **Socket bind duplicate checking** -- TCP/UDP return EADDRINUSE on port
   conflicts
4. **Cgroups v2: 4 bugs fixed** -- dead PID leak in member_pids, recursive
   spinlock hold, non-atomic migration, O_CREAT on control files

Test results: **66/66 PASS, 0 FAIL** (up from 61).

## utimes/utimensat: real file timestamps

### The problem

`utimes(2)` and `utimensat(2)` were stubs -- they verified the file existed
but never modified timestamps. This broke `touch`, `make` (dependency
tracking), and APK's package management metadata.

### The fix

Added `set_times(atime_secs, mtime_secs)` method to the VFS trait hierarchy:
- `FileLike`, `Directory`, `Symlink` traits (with default no-op)
- `INode` enum dispatcher
- Ext4 implementation: locks inode, updates atime/mtime/ctime, calls
  `write_inode()` to persist to disk

Rewrote both syscalls:
- **`utimes`**: parses `struct timeval[2]`, calls `set_times()`
- **`utimensat`**: parses `struct timespec[2]`, handles `UTIME_NOW`,
  `UTIME_OMIT`, `AT_SYMLINK_NOFOLLOW`, fd-based operation via `CwdOrFd`

Uses `read_wall_clock().secs_from_epoch()` for `UTIME_NOW` and `times==NULL`.

**Files:** `libs/kevlar_vfs/src/inode.rs`, `libs/kevlar_vfs/src/stat.rs`,
`services/kevlar_ext2/src/lib.rs`, `kernel/syscalls/utimes.rs`,
`kernel/syscalls/utimensat.rs`

## flock(2): advisory file locking

### The problem

`flock(2)` was a no-op stub -- it validated the fd and returned success. APK,
build tools, and databases rely on advisory locking for coordination.

### The fix

Global lock table keyed by `(dev_id, inode_no)` with per-open-file-description
(OFD) tracking. The OFD identity is the raw `Arc<OpenedFile>` pointer, so
fork'd children sharing the same file description share the lock.

Operations:
- `LOCK_SH` -- shared lock (multiple readers)
- `LOCK_EX` -- exclusive lock (single writer)
- `LOCK_UN` -- explicit unlock
- `LOCK_NB` -- non-blocking (returns EAGAIN on contention)
- Upgrade (SH -> EX) and downgrade (EX -> SH) supported
- Auto-release via `Drop` on `OpenedFile` when last Arc reference drops

**Files:** `kernel/syscalls/flock.rs`, `kernel/syscalls/mod.rs`,
`kernel/fs/opened_file.rs`

## Socket bind duplicate port checking

### The problem

TCP and UDP `bind()` silently allowed duplicate port binds. Services like
nginx, sshd, and dropbear expect `EADDRINUSE` when a port is taken.

### The fix

- **TCP:** Check `INUSE_ENDPOINTS` set before bind, insert on success, remove
  in `Drop`
- **UDP:** Reject non-zero port duplicates (random port assignment already
  skipped in-use ports), added `Drop` impl to release port and smoltcp handle

**Files:** `kernel/net/tcp_socket.rs`, `kernel/net/udp_socket.rs`

## Cgroups v2: 4 bugs fixed

### Bug 1 (critical): dead PID leak in member_pids

`Process::exit()` never removed the dying process's PID from its cgroup's
`member_pids` list. Dead PIDs accumulated indefinitely, causing:
- Inflated `pids.current` count
- `cgroup.procs` listing dead PIDs
- `rmdir` failing on emptied cgroups (EBUSY)
- Fork failures if `pids.max` was set (EAGAIN from inflated count)

**Fix:** Added `cg.member_pids.lock().retain(|p| *p != current.pid)` before
`set_state(ExitedWith)` in `Process::exit()`.

### Bug 2: recursive spinlock hold in count_pids_recursive

`count_pids_recursive()` held `self.children.lock()` across recursive calls
into child cgroups. Under concurrent fork + cgroup.procs writes, this created
prolonged lock contention.

**Fix:** Collect children into a Vec under lock, then release lock before
recursing:

```rust
let children: Vec<Arc<CgroupNode>> = self.children.lock().values().cloned().collect();
children.iter().fold(count, |acc, child| acc + child.count_pids_recursive())
```

### Bug 3: non-atomic cgroup.procs migration

Writing to `cgroup.procs` removed the PID from the old cgroup and added to
the new in two separate lock acquisitions. Between them, the PID was in
neither cgroup.

**Fix:** Lock both cgroups atomically in pointer order to prevent deadlock:

```rust
if old_ptr < new_ptr {
    let mut old_pids = old_cgroup.member_pids.lock();
    let mut new_pids = self.node.member_pids.lock();
    // migrate atomically
}
```

### Bug 4: O_CREAT on cgroupfs control files returned EPERM

BusyBox shell's `echo 0 > cgroup.procs` uses `open(path, O_WRONLY|O_CREAT|O_TRUNC)`.
The kernel's open path calls `create_file()` first when `O_CREAT` is set. If
it returns `EEXIST`, open falls through to the existing-file lookup. But
`CgroupDir::create_file()` returned `EPERM` unconditionally, which didn't
match `EEXIST` and propagated as an error.

**Fix:** Return `EEXIST` for names that match existing control files or child
cgroup directories.

### Bonus: PID 0 -> current process in cgroup.procs write

Writing "0" to `cgroup.procs` is the standard Linux way to move the current
process. The handler now maps PID 0 to `current_process().pid()`.

**Files:** `kernel/process/process.rs`, `kernel/cgroups/mod.rs`,
`kernel/cgroups/cgroupfs.rs`

## Test results

**66/66 PASS, 0 FAIL:**

| Category | Tests | Status |
|---|---|---|
| Boot + OpenRC | 3 | PASS |
| Cgroups v2 | 2 | PASS (NEW) |
| APK package management | 3 | PASS |
| curl HTTP/HTTPS | 3 | PASS |
| ext4 filesystem | 18 | PASS |
| File timestamps | 2 | PASS (NEW) |
| Advisory locking | 4 | PASS (NEW) |
| Dynamic linking | 5 | PASS |
| dlopen from C | 6 | PASS |
| mmap integrity | 4 | PASS |
| Long symlinks | 5 | PASS |
| Python 3.12 | 7 | PASS |
| **Total** | **66** | **ALL PASS** |

## Benchmark results (Kevlar vs Linux KVM)

**0 regressions, 23 faster, 21 at parity** across 44 micro-benchmarks:

| Benchmark | Kevlar (ns) | Linux (ns) | Ratio | Verdict |
|---|---|---|---|---|
| getpid | 70 | 101 | 0.69x | FASTER |
| gettid | 1 | 102 | 0.01x | FASTER (vDSO) |
| clock_gettime | 12 | 22 | 0.55x | FASTER (vDSO) |
| brk | 6 | 2620 | 0.00x | FASTER |
| mmap_fault | 89 | 1805 | 0.05x | FASTER |
| mmap_munmap | 341 | 1699 | 0.20x | FASTER |
| socketpair | 971 | 2596 | 0.37x | FASTER |
| file_tree | 37337 | 74965 | 0.50x | FASTER |
| open_close | 642 | 792 | 0.81x | FASTER |
| exec_true | 91289 | 111204 | 0.82x | FASTER |
| shell_noop | 121580 | 156343 | 0.78x | FASTER |
| fork_exit | 59456 | 57152 | 1.04x | parity |
| tar_extract | 723270 | 641299 | 1.13x | parity |

## Full regression suite

All test suites pass with zero regressions:

| Suite | Tests | Status |
|---|---|---|
| Alpine APK (ext4 + curl + Python + dlopen) | 66/66 | PASS |
| ext4 comprehensive | 42/42 | PASS |
| BusyBox applets | 100/100 | PASS |
| SMP threading (4 CPUs) | 14/14 | PASS |
| SMP regression (mini_systemd) | 16/16 | PASS |
| Cgroups + namespaces | 14/14 | PASS |
| VM contract tests | 20/20 | PASS |

## OpenRC boot investigation

With the cgroups fixes, **OpenRC itself now boots successfully** — all three
runlevels (sysinit, boot, default) complete with empty service lists.

However, **individual service startup via `openrc-run` hangs** after the
service function completes. The service itself succeeds (e.g., "Setting
hostname ... [ok]") but `openrc-run` never exits. This affects all services
tested: hostname, cgroups, bootmisc, seedrng.

The hang is NOT caused by:
- fd inheritance (redirecting all fds to /dev/null doesn't help)
- The `timeout` command (hang persists without timeout)
- cgroups PID accounting (fixed in this session)
- cgroupfs O_CREAT (fixed in this session)

Detailed investigation found two issues:

**Issue 1 (FIXED): Pipe close never woke POLL_WAIT_QUEUE.** The pipe
implementation only woke its local `waitq` on state changes, not the global
`POLL_WAIT_QUEUE` used by `poll(2)`. Added `POLL_WAIT_QUEUE.wake_all()` to
all 7 pipe wake points (PipeWriter/PipeReader read/write/drop).

**Issue 2 (IDENTIFIED): `openrc-run` self-pipe SIGCHLD pattern.** OpenRC uses
`posix_spawn` (falls back to fork+exec on musl) with a self-pipe pattern:
- Creates `pipe2(signal_pipe, O_CLOEXEC)`
- Forks child to run service
- SIGCHLD handler in parent calls `waitpid` + `write(signal_pipe[1])`
- Parent does `poll(signal_pipe[0], POLLIN, -1)` to detect child exit

The parent blocks in `poll()` waiting for POLLIN. When SIGCHLD arrives,
`poll()` returns EINTR, the signal handler runs, writes to the pipe, and
the re-entered `poll()` sees POLLIN. Syscall tracing confirmed the
`openrc-run` parent process (running /sbin/openrc) is stuck in an ioctl
loop querying terminal window size — suggesting the SIGCHLD/poll/handler
chain works but a subsequent output formatting step loops.

5. **Root cause: cgroupfs `poll()` not implemented (FIXED).** Instrumented
   the openrc-run.sh shell script and traced the hang to `while read ...
   done < cgroup.events`. The `CgroupControlFile` type used the default
   `FileLike::poll()` which returns empty events. BusyBox ash calls `poll()`
   on file descriptors before reading from shell redirects (`< file`). With
   empty poll events, ash blocks forever. Fix: implement `poll()` returning
   `POLLIN | POLLOUT` (matching regular file behavior).

6. **cgroupfs `read()` offset handling (FIXED).** The read implementation
   ignored the offset parameter. Fixed to respect position for sequential
   reads.

**Result:** OpenRC now boots Alpine with real services — hostname, cgroups,
and seedrng all start successfully.

## What's next

1. Integrate full OpenRC boot into the main Alpine test suite
2. Test more Alpine packages (gcc, make, git, openssh, nginx)
3. ARM64 testing with updated kernel
