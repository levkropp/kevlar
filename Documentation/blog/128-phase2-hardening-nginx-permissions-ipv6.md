# Blog 128: Phase 2 hardening — nginx, file permissions, IPv6, /proc fixes

**Date:** 2026-03-29
**Milestone:** M10 Alpine Linux — Phase 2 Complete

## Summary

Final hardening pass before Phase 3, closing infrastructure gaps and
validating production network services:

1. **nginx 4/4 PASS** — install via apk, config validates, daemon starts,
   listening on port 80
2. **File permission enforcement** — DAC checks in open(), openat(), execve()
   against euid/egid with root bypass
3. **AF_INET6 graceful degradation** — socket(AF_INET6) returns EAFNOSUPPORT
   so programs fall back to IPv4
4. **/proc/net/tcp port fix** — listening sockets now show actual bound port
   via smoltcp `listen_endpoint()`
5. **/proc/sys writeback** — mutable tunables persist writes for read-after-write
   consistency

## nginx integration test

### Setup

The test follows the Alpine APK test pattern: boot Alpine ext4 rootfs,
install nginx via `apk.static add`, start the daemon, verify it's running.

### IPv6 workaround

Alpine's default nginx config includes `listen [::]:80;` for IPv6. Since
Kevlar doesn't implement AF_INET6, this causes:
```
nginx: [emerg] socket() [::]:80 failed (97: Address family not supported by protocol)
```

The test patches this out with `sed -i 's/listen.*\[::\].*;//g'` before
starting nginx. Once IPv6 is implemented, this workaround can be removed.

### Results

| Test | Result |
|------|--------|
| nginx install (apk add nginx) | PASS |
| nginx config validate (nginx -t) | PASS |
| nginx daemon running (kill -0 pid) | PASS |
| Port 80 listening (/proc/net/tcp) | PASS |

### Makefile target

```
make test-nginx    # Requires build/alpine.img
```

## File permission enforcement

### What changed

Added DAC (Discretionary Access Control) permission checks to three
critical syscall paths:

**open() / openat():** After inode resolution, check R_OK/W_OK against
the file's mode bits and the process's effective UID/GID:
```rust
let want = match flags.bits() & 0o3 {
    O_RDONLY => R_OK,
    O_WRONLY => W_OK,
    O_RDWR   => R_OK | W_OK,
    _ => 0,
};
check_access(&stat, current.euid(), current.egid(), want)?;
```

**execve():** Before loading the ELF binary, verify X_OK (execute
permission) on the file:
```rust
let stat = executable.inode.stat()?;
check_access(&stat, current.euid(), current.egid(), X_OK)?;
```

### Root bypass

The existing `check_access()` function (in `kernel/fs/permission.rs`)
bypasses all checks when `euid == 0`. Since all current processes run as
root, this change has zero impact on existing tests. Permission enforcement
activates when non-root users are introduced (Phase 7: multi-user security).

### What it enables

- Non-root processes can't read files with mode 0600 owned by root
- Non-root processes can't execute files without the execute bit
- Non-root processes can't write to read-only files
- Standard Unix security model for multi-user Alpine operation

## AF_INET6 graceful degradation

Added `AF_INET6 = 10` constant and explicit match arm in `sys_socket()`:
```rust
(AF_INET6, _, _) | (AF_PACKET, _, _) => {
    Err(Errno::EAFNOSUPPORT.into())
}
```

Previously, AF_INET6 fell through to the default arm which logged a
`debug_warn!()` on every call. The explicit arm is silent — IPv6 socket
creation failures are expected and handled by all well-written programs
(musl, curl, nginx, dropbear all try IPv6 first and fall back to IPv4).

## /proc/net/tcp port fix

### The problem

Listening TCP sockets showed `00000000:0000` for local address because
smoltcp's `tcp.local_endpoint()` returns `None` for sockets in LISTEN state
(no connection established yet).

### The fix

Use `tcp.listen_endpoint()` as fallback, which returns the
`IpListenEndpoint { addr: Option<IpAddress>, port: u16 }` from the
socket's bind configuration:

```rust
let local_str = match tcp.local_endpoint() {
    Some(ep) => ip_endpoint_to_hex(&ep),
    None => {
        let lep = tcp.listen_endpoint();
        listen_endpoint_to_hex(lep.addr, lep.port)
    }
};
```

Now `ss` and `netstat` correctly show `0.0.0.0:22` for dropbear and
`0.0.0.0:80` for nginx.

## /proc/sys mutable tunables

### The problem

`ProcSysStaticFile` accepted writes silently but always returned the
original value on subsequent reads. Programs that write then read back
(e.g., systemd testing sysctl support) would see stale values.

### The fix

New `ProcSysMutableFile` type with a `SpinLock<String>` that persists
the last written value:

Applied to: `overcommit_memory`, `max_map_count`, `ip_forward`,
`tcp_syncookies`. Other tunables remain static (writes accepted, reads
return default).

## Phase 2 completion status

All Phase 2 (Network Services) items are now complete or deferred:

| Item | Status |
|------|--------|
| SO_REUSEADDR enforcement | Done |
| SO_KEEPALIVE / TCP_NODELAY | Done |
| SO_RCVTIMEO / SO_SNDTIMEO | Done |
| SSH (Dropbear) | Done (3/3 tests) |
| nginx | Done (4/4 tests) |
| AF_INET6 | Graceful degradation (EAFNOSUPPORT) |
| File permissions | Done (DAC in open/openat/execve) |
| /proc/net/tcp ports | Done |
| /proc/sys writeback | Done |

**Ready for Phase 3: Build & Package Ecosystem.**

## Files changed

| Area | Files |
|------|-------|
| Permissions | `kernel/syscalls/open.rs`, `openat.rs`, `execve.rs` |
| IPv6 | `libs/kevlar_vfs/src/socket_types.rs`, `kernel/syscalls/socket.rs` |
| /proc | `kernel/fs/procfs/mod.rs`, `kernel/net/mod.rs` |
| nginx test | `testing/test_nginx.c`, `Makefile`, `tools/build-initramfs.py` |
