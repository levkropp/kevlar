# Blog 116: OpenSSL, TLS 1.3, curl HTTPS — full crypto stack on Alpine/Kevlar

**Date:** 2026-03-24
**Milestone:** M10 Alpine Linux

## Summary

Five kernel bugs fixed, an 18-layer OpenSSL/TLS test suite built, and the full
crypto stack now works on Alpine 3.21 running on Kevlar: OpenSSL 3.3.6 with
TLS 1.3 (AES-256-GCM-SHA384), curl HTTP and HTTPS with full certificate
verification, and c-ares native DNS resolution. All 18 OpenSSL tests pass,
159/159 contract tests pass, 7/7 M10 APK tests pass.

## Bugs Fixed

### 1. Mount namespace not shared across fork (kernel/fs/mount.rs)

Fork deep-cloned `mount_points` as a `Vec`, so mounts done by child processes
(like `busybox mount -t ext2 /dev/vda /mnt`) were invisible to the parent.
When the mount command exited, its mount was lost. The parent's subsequent
`mkdir -p /mnt/proc` hit the read-only initramfs and got EROFS.

**Fix:** Changed `mount_points` from `Vec<(MountKey, MountPoint)>` to
`Arc<SpinLock<Vec<(MountKey, MountPoint)>>>`. Fork clones the Arc (sharing
the mount namespace per POSIX), while cwd/root remain per-process via
independent String/Arc clones.

This was the fundamental blocker for the M10 APK test (went from 2/7 to 7/7).

### 2. utimensat ignored dirfd (kernel/syscalls/utimensat.rs)

The `dirfd` parameter was unused — relative paths like
`usr/lib/.apk.52fbde...` resolved from cwd instead of the directory fd.
apk's package extraction uses `utimensat(dirfd, "relative-temp-name", ...)`
to set modification times, producing "Failed to preserve modification time"
errors for all 9 installed packages.

**Fix:** Use `lookup_path_at()` with the dirfd parameter. Also handle
`AT_EMPTY_PATH` flag (operate directly on the fd).

### 3. Fast symlink unlink returned EIO (services/kevlar_ext2/src/lib.rs)

`free_file_blocks()` interpreted fast symlink inline `block[]` data (the
symlink target string stored directly in the inode) as block pointers. For
a symlink to `/usr/lib/libfoo.so`, the bytes `2f 75 73 72 2f 6c 69 62`
became "block numbers" `0x7273752f`, `0x62696c2f`, etc. Trying to free
these garbage addresses returned EIO.

**Fix:** Skip `free_file_blocks()` for fast symlinks
(`is_symlink() && blocks == 0`) — they have no data blocks to free.

### 4. Missing UDP getsockname (kernel/net/udp_socket.rs)

`UdpSocket` didn't implement `getsockname()` — the default `FileLike` trait
returned EBADF. c-ares (curl's DNS resolver) calls `getsockname()` after
connecting its UDP socket to determine the local address. Getting EBADF,
c-ares marks the DNS server as dead and refuses all queries, causing curl's
"Could not resolve hostname" error.

**Root cause diagnosis:** Built an LD_PRELOAD tracing library (`trace_sock.c`)
that intercepted all socket syscalls from c-ares. The trace showed:
```
socket(AF_INET, SOCK_DGRAM, 0) = 6
connect(fd=6, 10.0.2.3:53) = 0
getsockname(fd=6) = -1 errno=9    <-- EBADF!
```

With custom `ares_set_socket_functions_ex()` interceptors that bypassed the
default socket path, c-ares resolved successfully — confirming the issue was
in the kernel's getsockname, not in c-ares's DNS logic.

**Fix:** Implemented `getsockname()` for UDP sockets (reads local endpoint
from smoltcp's socket state) and `getpeername()` (returns the connected
peer from the socket's stored peer address).

### 5. utimensat AT_EMPTY_PATH not handled

Fixed alongside the dirfd bug. `AT_EMPTY_PATH` (0x1000) tells `utimensat`
to operate on the open file descriptor itself, not a path. Without handling
this flag, programs that set timestamps on already-open fds would fail.

## OpenSSL/TLS Test Suite

Built `test_openssl.c` — an 18-test incremental suite compiled against
Alpine's libcrypto/libssl/libcurl. Each layer depends on the previous,
isolating exactly where Kevlar diverges from Linux.

| Layer | Tests | What It Validates |
|-------|-------|-------------------|
| L1 | getrandom, /dev/urandom | Kernel entropy sources |
| L2 | OpenSSL_version, RAND_status, RAND_bytes | OpenSSL 3.3.6 DRBG initialization |
| L3 | SHA-256, AES-256-CBC | Crypto primitives |
| L4 | SSL_CTX_new, CA bundle (146 certs) | TLS context + trust store |
| L5 | resolv.conf, getaddrinfo | DNS resolution |
| L6 | TCP connect + HTTP GET | Raw socket networking |
| L7 | SSL_connect (TLS 1.3, AES_256_GCM_SHA384) | TLS handshake |
| L8 | SSL_VERIFY_PEER (google.com, full chain) | Certificate verification |
| L9 | HTTPS GET via raw OpenSSL (200 OK) | End-to-end TLS |
| L9b | curl without CURLOPT_RESOLVE | c-ares native DNS |
| L10 | curl HTTP (200 OK, 528 bytes) | libcurl HTTP |
| L11 | curl HTTPS no verify (200 OK) | libcurl TLS |
| L12 | curl HTTPS full verification (google.com) | libcurl + cert chain |

**Result: 18/18 PASS.**

### Build infrastructure

The test binary is compiled inside an Alpine environment (bwrap sandbox with
Alpine minirootfs) against Alpine's `-lcurl -lssl -lcrypto` headers. It runs
inside the Alpine ext4 rootfs after pivot_root, with OpenRC-style networking.

```
make test-openssl   # Boots Alpine, runs 18-layer TLS test suite
```

## Diagnostic Tooling Built

- **`trace_sock.c`** — LD_PRELOAD shared library that wraps socket/bind/
  connect/sendto/recvfrom/setsockopt/getsockopt/getsockname with stderr
  tracing. Used to pinpoint the getsockname EBADF root cause.
- **`test_cares_diag.c`** — Direct c-ares diagnostic: tests IPv6 socket
  probe, pthread creation, ares_init, manual UDP DNS, threaded UDP DNS,
  c-ares with custom socket functions, and c-ares default path.
- **`test_openssl_boot.c`** — Boot shim that mounts ext4, sets up networking,
  pivot_roots into Alpine, and runs the test binary.

## Status

| Suite | Result |
|-------|--------|
| Contract tests | **159/159 PASS** |
| M10 APK (ext2) | **7/7 PASS** |
| ext4 comprehensive | **29/29 PASS** |
| OpenSSL/TLS | **18/18 PASS** |

### What's working on Alpine 3.21/Kevlar

- OpenRC boot (sysinit + boot + default runlevels)
- apk package manager (25,397 packages available)
- curl HTTP and HTTPS with full TLS 1.3 + certificate verification
- GCC compiles and runs programs
- c-ares native DNS resolution
- ext4 filesystem (2.6x faster writes than Linux)
- Dynamic linking (musl libc + all shared libraries)

### Remaining gaps

- **Blocking TCP connect():** `connect()` on blocking sockets doesn't
  honor `SO_SNDTIMEO` — must use `SOCK_NONBLOCK` + `poll()` + `connect()`.
  Works but not Linux-identical behavior.
- **example.com cert chain:** Cloudflare serves a chain terminating at
  "AAA Certificate Services" (old Comodo root) not in Alpine 3.21's CA
  bundle. Same failure on host Linux. Not a Kevlar issue.
