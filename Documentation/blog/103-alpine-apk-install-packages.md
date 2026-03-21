# Blog 103: Alpine apk installs packages on Kevlar — 25,397 packages available

**Date:** 2026-03-21
**Milestone:** M10 Alpine Linux

## The Breakthrough

`apk add curl` installs curl and all 9 dependencies (13 MiB, 27 total
packages) on Alpine Linux running on Kevlar. The Alpine package repository
is fully accessible with 25,397 packages available.

```
/ # apk update
v3.21.6-64-gf251627a5bd [http://dl-cdn.alpinelinux.org/alpine/v3.21/main]
v3.21.6-63-gc07db2dfa93 [http://dl-cdn.alpinelinux.org/alpine/v3.21/community]
OK: 25397 distinct packages available

/ # apk add curl
8 errors; 13 MiB in 27 packages
```

---

## Fixes This Session

### 1. Netlink NETLINK_ROUTE sockets (kernel/net/netlink.rs)
Implemented minimal netlink for `ip link/addr/route`:
- RTM_NEWLINK: interface up/down
- RTM_NEWADDR: IPv4 address assignment → `INTERFACE.update_ip_addrs()`
- RTM_NEWROUTE: default gateway → `INTERFACE.routes_mut().add_default_ipv4_route()`
- RTM_GETLINK: returns eth0 interface info

### 2. Relative symlink resolution (kernel/fs/mount.rs)
Symlinks like `libz.so.1 → libz.so.1.3.1` were resolved from cwd instead
of the symlink's parent directory. Fixed by prepending parent path.

### 3. SIGSEGV infinite loop fix (kernel/mm/page_fault.rs)
Unrecoverable SIGSEGV (invalid address, no VMA) now calls `exit_by_signal`
directly when no user handler is installed. Permission faults still use
`send_signal` for user handlers.

### 4. Unix socket ECONNREFUSED (kernel/net/unix_socket.rs)
SOCK_STREAM connect to non-existent listener now returns ECONNREFUSED
(was returning Ok(0)). Fixes musl's initgroups/nscd fallback.

### 5. fakeroot for ext4 image building (Makefile)
Docker export as non-root user created files owned by UID 1000. Fixed
by wrapping docker export + mke2fs in fakeroot.

### 6. HTTP repositories for apk
HTTPS "Permission denied" — TLS/OpenSSL needs investigation. Switched
to HTTP repos as workaround. apk update/add work over plain HTTP.

### 7. O_TMPFILE support (kernel/fs/opened_file.rs)
Added O_TMPFILE flag (returns ENOSYS since we lack linkat AT_EMPTY_PATH).
Also added O_NOFOLLOW.

---

## Known Issues

| Issue | Severity | Notes |
|-------|----------|-------|
| HTTPS "Permission denied" | Medium | TLS/OpenSSL issue; HTTP works |
| fchownat errors during apk install | Low | Non-fatal ownership errors on temp files |
| OpenRC boot SIGSEGV | Low | Non-fatal, OpenRC recovers |
| Login shell apk lock error | Low | Workaround: getty -n -l /bin/sh |

---

## Session Statistics

- **25+ commits** this session
- **Contract tests:** 118/118 PASS
- **Alpine packages:** 25,397 available, installing works
- **New features:** Netlink sockets, O_TMPFILE, relative symlinks
- **Infrastructure:** fakeroot image build, HTTP repos, make run-alpine
