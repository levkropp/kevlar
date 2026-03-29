# Blog 126: Phase 1 Core POSIX gaps -- sessions, fcntl locks, statx, rlimits, /proc

**Date:** 2026-03-29
**Milestone:** M10 Alpine Linux -- Phase 1 (Core POSIX Gaps)

## Summary

Seven improvements closing fundamental POSIX gaps identified in the
Alpine drop-in compatibility audit:

1. **statx timestamps fixed** -- returns real atime/mtime/ctime from inode
2. **File creation timestamps** -- ext4 files/dirs now get current time on create
3. **Session tracking** -- session_id in Process, proper setsid/getsid/TIOCSCTTY
4. **fcntl record locks** -- F_SETLK/F_GETLK/F_SETLKW with byte-range lock table
5. **/proc/[pid]/cwd,root,limits** -- three missing per-process proc files
6. **/proc/net/tcp,udp real data** -- enumerate actual smoltcp sockets
7. **setrlimit with per-process storage** -- rlimits stored, inherited, enforced

These collectively unblock: SSH daemonization (sessions), sqlite/database
ACID (record locks), find/rsync/make (timestamps), and monitoring tools
like lsof/ss/top (/proc gaps).

## 1. statx: real timestamps from inode

### The problem

`statx(2)` returned hardcoded zero timestamps for all fields (atime, mtime,
ctime, btime), even though the underlying inode had real values from
`utimes`/`utimensat`. Also returned hardcoded 1 for nlink and 0 for uid/gid.

### The fix

`kernel/syscalls/statx.rs`: Copy all fields from the `Stat` struct returned
by `inode.stat()` into the `StatxBuf`:

```rust
stx_atime: StatxTimestamp { tv_sec: stat.atime.as_isize() as i64, ... },
stx_mtime: StatxTimestamp { tv_sec: stat.mtime.as_isize() as i64, ... },
stx_ctime: StatxTimestamp { tv_sec: stat.ctime.as_isize() as i64, ... },
stx_nlink: stat.nlink.as_usize() as u32,
stx_uid: stat.uid.as_u32(),
stx_gid: stat.gid.as_u32(),
stx_blocks: stat.blocks.as_isize() as u64,
```

Added `as_isize()`, `as_usize()` getters to `Time`, `NLink`, `BlockCount`
in `kevlar_vfs/src/stat.rs`.

## 2. File creation timestamps

### The problem

Ext4's `create_file` and `create_dir` initialized all timestamps to 0
(epoch 1970-01-01). `ls -la` showed every file created at the dawn of Unix.

### The fix

After `create_file`/`create_dir` returns the new inode, the kernel syscall
layer calls `set_times(now, now)` with the current wall clock:

- `kernel/syscalls/open.rs` (O_CREAT path)
- `kernel/syscalls/openat.rs` (O_CREAT path)
- `kernel/syscalls/mkdir.rs`
- `kernel/syscalls/mkdirat.rs`

This keeps timer dependencies in the kernel crate (ext4 service crate
doesn't need to import the clock).

## 3. Session tracking

### The problem

No session concept existed. `getsid()` returned the process group ID.
`setsid()` created a new process group but never tracked the session.
`TIOCSCTTY` was a no-op. `/proc/[pid]/stat` reported the PID itself for
both pgrp and session fields.

### The fix

Added `session_id: AtomicI32` to the `Process` struct:

- **Idle thread**: session_id = 0
- **Init (PID 1)**: session_id = 1 (session leader)
- **fork/vfork/clone**: inherit parent's session_id
- **setsid()**: sets session_id = caller's PID (becomes session leader)
- **getsid()**: returns actual session_id
- **TIOCSCTTY**: sets foreground process group to caller's group
- **TIOCGSID**: returns actual session_id
- **/proc/[pid]/stat**: fields 5 (pgrp) and 6 (session) now report real values

This unblocks `getty`, `login`, SSH daemonization, and proper job control.

## 4. fcntl record locks (F_SETLK/F_GETLK/F_SETLKW)

### The problem

`fcntl(2)` only supported file descriptor operations (F_DUPFD, F_GETFD,
F_SETFD, F_GETFL, F_SETFL). POSIX record locks (F_SETLK/F_GETLK/F_SETLKW)
returned ENOSYS. This breaks sqlite WAL mode, postgresql, and any
application using `lockf()`.

### The fix

Full byte-range record lock implementation in `kernel/syscalls/fcntl.rs`:

- **Lock table**: global `HashMap<InodeKey, Vec<RecordLock>>` keyed by
  (dev_id, inode_no)
- **RecordLock**: `{ start: u64, end: u64, l_type: i16, pid: i32 }`
- **F_GETLK**: checks for conflicts, returns conflicting lock info or F_UNLCK
- **F_SETLK**: non-blocking acquire -- checks conflicts, splits/merges ranges
- **F_SETLKW**: returns EAGAIN (no real blocking yet, like flock)
- **Conflict rules**: write locks conflict with everything; read locks only
  conflict with write locks; same PID can overlap its own locks
- **Range operations**: set_lock() properly trims/splits existing locks when
  a new lock overlaps partial ranges
- **Cleanup**: `release_all_record_locks(pid)` called from `Process::exit()`

Struct flock ABI (x86_64, 32 bytes):
```
offset 0: l_type (i16)    -- F_RDLCK=0, F_WRLCK=1, F_UNLCK=2
offset 2: l_whence (i16)  -- SEEK_SET/SEEK_CUR/SEEK_END
offset 8: l_start (i64)
offset 16: l_len (i64)    -- 0 means to EOF
offset 24: l_pid (i32)
```

## 5. /proc/[pid]/cwd, root, limits

### The problem

Tools like `lsof`, `fuser`, `ps`, and `top` read `/proc/[pid]/cwd` (current
directory symlink), `/proc/[pid]/root` (root directory symlink), and
`/proc/[pid]/limits` (resource limits). All returned ENOENT.

### The fix

Added three entries to `ProcPidDir::lookup()` in `proc_self.rs`:

- **cwd**: symlink resolved from `process.root_fs().lock().cwd_path()`
- **root**: symlink always pointing to `/` (no chroot support yet)
- **limits**: formatted file matching Linux's `/proc/[pid]/limits` layout
  with all 16 RLIMIT_* entries

## 6. /proc/net/tcp,udp with real socket data

### The problem

`/proc/net/tcp` and `/proc/net/udp` were static files that returned only
the header line. `ss`, `netstat`, and monitoring tools saw zero sockets.

### The fix

Two new dynamic file types (`ProcNetTcpFile`, `ProcNetUdpFile`) in
`kernel/fs/procfs/system.rs` that call helper functions in `kernel/net/mod.rs`:

- **format_proc_net_tcp()**: iterates `SOCKETS.lock().iter()`, matches
  `Socket::Tcp`, formats local/remote endpoints as hex + TCP state code
- **format_proc_net_udp()**: same for `Socket::Udp` with listen endpoints

TCP state mapping follows Linux conventions (ESTABLISHED=01, SYN_SENT=02,
..., LISTEN=0A, CLOSING=0B).

IP addresses formatted as `AABBCCDD:PORT` using `Ipv4Addr::octets()`.

## 7. setrlimit with per-process rlimit storage

### The problem

`getrlimit()` returned hardcoded values. `setrlimit()` didn't exist.
`prlimit64()` ignored writes. Daemons that set fd limits, stack sizes,
or core dump settings had no effect.

### The fix

Added `rlimits: SpinLock<[[u64; 2]; 16]>` to the Process struct:

- **16 resources** indexed by RLIMIT_* constants, each with [cur, max]
- **Defaults**: STACK=8MB/INF, NOFILE=1024/4096, CORE=0/INF, rest=INF
- **Inheritance**: fork/vfork/clone copy parent's rlimits
- **getrlimit**: reads from process rlimits table
- **setrlimit** (syscall 160, new): writes to process rlimits table
- **prlimit64**: now reads old AND writes new values (was read-only)

## Files changed

| Area | Files |
|------|-------|
| statx | `kernel/syscalls/statx.rs`, `libs/kevlar_vfs/src/stat.rs` |
| Timestamps | `kernel/syscalls/open.rs`, `openat.rs`, `mkdir.rs`, `mkdirat.rs` |
| Sessions | `kernel/process/process.rs`, `kernel/syscalls/setsid.rs`, `getsid.rs`, `kernel/fs/devfs/tty.rs`, `kernel/fs/procfs/proc_self.rs` |
| Record locks | `kernel/syscalls/fcntl.rs`, `kernel/syscalls/mod.rs` |
| /proc files | `kernel/fs/procfs/proc_self.rs`, `kernel/fs/procfs/system.rs`, `kernel/fs/procfs/mod.rs` |
| Socket enum | `kernel/net/mod.rs` |
| rlimits | `kernel/syscalls/getrlimit.rs`, `kernel/process/process.rs`, `kernel/syscalls/mod.rs` |
