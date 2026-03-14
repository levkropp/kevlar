# Phase 3.3: Service Startup — basic.target

**Duration:** 3-4 days (iterative)
**Prerequisite:** Phase 3.2 (systemd reaches sysinit.target)
**Goal:** systemd loads unit files, starts journald, reaches `basic.target`.

## What happens between sysinit.target and basic.target

After sysinit.target, systemd:
1. Starts `systemd-journald.service` (logging daemon)
2. Processes `sockets.target` (socket activation)
3. Mounts remaining filesystems from `/etc/fstab`
4. Reaches `basic.target` (essential system setup complete)

## journald Challenges

journald is the most likely blocker. It:
- Creates `/run/log/journal/<machine-id>/` directory
- Opens journal files with `fallocate()` + `mmap()`
- Uses `memfd_create` for sealed data
- Writes structured binary journal entries
- Listens on `/run/systemd/journal/socket` (AF_UNIX SOCK_DGRAM)

Potential fixes:
- `/run/log/journal/` must be writable (tmpfs at /run handles this)
- `fallocate` — already implemented
- `mmap` file-backed — may need fixes for journal files
- AF_UNIX SOCK_DGRAM — may need to add to socket.rs

## Expected Issues

| # | Issue | Fix |
|---|-------|-----|
| 1 | journald socket (SOCK_DGRAM) | Extend AF_UNIX with SOCK_DGRAM support |
| 2 | journal directory creation | Ensure /run/log/ is writable |
| 3 | mmap on tmpfs files | Verify tmpfs file mmap works |
| 4 | `/etc/fstab` parsing | Provide empty fstab |
| 5 | `setsockopt(SO_PASSCRED)` | May need credential passing |
| 6 | `getsockopt(SO_TYPE)` | Return correct socket type |
| 7 | `fcntl(F_SETPIPE_SZ)` | Stub returning 0 |

## AF_UNIX SOCK_DGRAM (if needed)

If journald requires SOCK_DGRAM:
- Extend `kernel/net/unix_socket.rs` with datagram mode
- Datagram: each `sendmsg()` is an independent message
- `bind()` creates a named endpoint
- No connect/listen/accept needed (connectionless)
- `recvmsg()` returns one datagram at a time

This is simpler than SOCK_STREAM since there's no connection state.

## Unit File Dependencies

Minimal unit files to get basic.target:
```
sysinit.target → basic.target
                 └─ systemd-journald.service (Type=notify)
```

## Testing

Capture serial output and grep for:
```
systemd[1]: Reached target Basic System.
```

## Success Criteria

- [ ] journald starts without crashing
- [ ] systemd reaches `basic.target`
- [ ] `basic.target` message appears in serial output
- [ ] No kernel panics
- [ ] All existing tests still pass
