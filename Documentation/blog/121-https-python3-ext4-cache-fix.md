# Blog 121: HTTPS/TLS works, Python3 runs, ext4 read cache staleness fix

**Date:** 2026-03-25
**Milestone:** M10 Alpine Linux

## Summary

Major Alpine compatibility advances in a single session:

1. **HTTPS/TLS 1.3** works via curl + OpenSSL on Alpine
2. **Python3** installs via `apk add` and runs pure Python code
3. **Ext4 read cache staleness** bug fixed — large package installs now work
4. **UDP getsockname** restored — fixes curl DNS via c-ares
5. **msync dispatch** restored — lost to git stash
6. **Kernel stack overflow** fixed by increasing to 8 pages (32KB)

## Ext4 read cache staleness (the big fix)

### Symptom

Installing Python3 via `apk add python3` failed with:
```
ERROR: python3-3.12.12-r0: failed to rename usr/lib/.apk.xxx to usr/lib/libpython3.12.so.1.0.
```

APK extracts package files to temporary names (`.apk.<hash>`) then renames
them to their final paths. The rename failed with ENOENT — the temp file
wasn't found, even though it was just created moments before.

### Root cause

The ext4 block I/O layer has a two-level cache:

1. **dirty_cache** (BTreeMap): blocks that have been written but not flushed
2. **read_cache** (Vec): blocks previously read from disk

`read_block()` checks dirty_cache first, then read_cache, then falls through
to disk. `write_block()` inserts into dirty_cache. When `flush_dirty()` fires
(dirty cache full), it writes all dirty blocks to disk and clears
dirty_cache — but **did not invalidate the read_cache**.

The race:
1. Block X read from disk → cached in read_cache (old data)
2. Block X modified (new directory entry added) → cached in dirty_cache
3. dirty_cache fills up during large install → `flush_dirty()` fires
4. dirty_cache cleared, blocks written to disk
5. Block X read again → dirty_cache miss, **read_cache hit with STALE data**

### Fix

Invalidate read_cache entries for flushed blocks in `flush_dirty()`:

```rust
fn flush_dirty(&self) -> Result<()> {
    let entries = core::mem::take(&mut *self.dirty_cache.lock_no_irq());
    // Invalidate stale read cache entries
    self.read_cache.lock_no_irq().retain(|e| !entries.contains_key(&e.block_num));
    // Write to disk...
}
```

This ensures subsequent reads go to disk and get the up-to-date data.

## UDP getsockname (re-applied)

The `getsockname()` and `getpeername()` implementations for UDP sockets were
lost to a git stash operation earlier. c-ares (curl's DNS resolver) calls
`getsockname()` after `connect()` on its UDP DNS socket. Without it, the
call returned EBADF, causing all DNS resolution to fail:

```
curl: (6) Could not resolve host: example.com
```

BusyBox wget worked because it uses musl's blocking DNS resolver (which
doesn't call getsockname on UDP sockets).

## Kernel stack overflow during TLS

### Symptom

curl HTTPS caused a kernel page fault at RIP=0x0 with all-zero registers and
stack. The crash was in kernel mode (CS=0x8, ring 0).

### Root cause

The x86_64 kernel stack was 4 pages (16KB) — matching Linux's default. But
Kevlar processes the entire TCP stack (smoltcp) inline during syscalls, unlike
Linux which handles TCP in separate kernel threads. The TLS handshake creates
deep call chains:

```
syscall → write → tcp_socket::sendto → smoltcp::dispatch →
  smoltcp::tcp::process → retransmit logic → ARP handling → ...
```

This exceeded the 16KB stack during the complex TLS handshake, overflowing
into unmapped memory (all zeros), causing the null function pointer call.

### Fix

Increased kernel stack to 8 pages (32KB). This is 2x Linux's default but
necessary because Kevlar's in-kernel networking has deeper call chains than
Linux's separate TCP processing model.

## HTTPS/TLS 1.3

With the stack fix, HTTPS works via curl + OpenSSL 3.3.6:
- DNS resolution via c-ares (UDP)
- TCP connection to port 443
- TLS 1.3 handshake (ECDHE key exchange, AES-256-GCM)
- Certificate verification (requires ca-certificates package)
- Encrypted data transfer

Currently tested with `-k` (skip cert verification) because
`update-ca-certificates` has symlink issues on our ext4. The TLS handshake
and encryption are the real kernel-level test.

## Python3

Python 3.12.12 installs via `apk add python3` (15 packages, ~291 MiB) and
runs pure Python code:

- `python3 --version` — interpreter loads correctly
- `print("hello")` — basic I/O works
- `import os; os.getpid()` — syscall interface works
- List comprehensions — bytecode execution works
- `import sys; sys.platform` — standard library loads

C extension modules (math, socket, hashlib) crash with SIGSEGV. This appears
to be related to `dlopen()` loading `.so` files at runtime. Tracked for
future investigation.

## Test results

- **Contract tests:** 159/159 PASS
- **Alpine APK tests:** all pass including:
  - curl HTTP (DNS + TCP)
  - curl HTTPS (TLS 1.3)
  - Python3 install + 5 pure Python tests
  - 29 ext4 filesystem tests
  - Dynamic linking tests (busybox, openrc, curl, apk, file)
