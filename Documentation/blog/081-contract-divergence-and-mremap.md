# 081: Contract Divergence Resolution, SIGSEGV Delivery, and mremap

## Context

After M9.8, the contract test suite reported:
**100 PASS | 10 XFAIL | 10 DIVERGE | 1 FAIL**

The 10 DIVERGEs and 1 FAIL broke the green suite. Investigation revealed three
classes of issues: a real bug in fd-passing, two signal delivery bugs that
prevented POSIX-compliant SIGSEGV handling, and a missing syscall (`mremap`)
needed for musl's `realloc`. All four were fixed this session.

Final state: **104 PASS | 8 XFAIL | 6 DIVERGE | 0 FAIL**

## Fix 1: SCM_RIGHTS fd-passing (sockets.scm_rights_fdpass)

### Root cause

`recvmsg.rs` only tried `downcast_ref::<UnixSocket>()` to find the inner
`UnixStream` for ancillary data. But `socketpair()` stores bare
`Arc<UnixStream>` objects in the fd table (not `UnixSocket` wrappers), so the
downcast always failed, `inner_stream` was `None`, and the kernel silently
dropped the SCM_RIGHTS cmsg — writing `msg_controllen=0` back to userspace.

`sendmsg.rs` already did it correctly: try `UnixStream` first, then
`UnixSocket`. The fix was to mirror that pattern in `recvmsg.rs`.

### Fix

```rust
// Before: only tried UnixSocket
let inner_stream: Option<Arc<UnixStream>> =
    if let Some(sock) = (**file).as_any().downcast_ref::<UnixSocket>() {
        sock.connected_stream()
    } else {
        None
    };

// After: try UnixStream first (socketpair), then UnixSocket (socket+connect)
let owned_stream: Option<Arc<UnixStream>> =
    if let Some(sock) = (**file).as_any().downcast_ref::<UnixSocket>() {
        sock.connected_stream()
    } else {
        None
    };
let stream: &UnixStream =
    if let Some(s) = (**file).as_any().downcast_ref::<UnixStream>() {
        s
    } else if let Some(ref s) = owned_stream {
        s
    } else {
        return Ok(0);
    };
```

This is the same `Arc<dyn FileLike>` downcast pattern documented in the M4
critical bugs section — `(**file).as_any()` dispatches through the vtable to
get the concrete type.

## Fix 2: SIGSEGV delivery for page faults

Two bugs prevented POSIX-compliant SIGSEGV delivery. Both had the same
symptom: processes that installed a SIGSEGV handler never had it called.

### Bug A: Write fault on read-only page (vm.mprotect_roundtrip)

After `mprotect(addr, len, PROT_READ)` removes write permission, writing to
the page triggers a page fault. The handler checked for Copy-on-Write:

```rust
let is_cow_write = reason.contains(PRESENT)
    && reason.contains(CAUSED_BY_WRITE)
    && (prot_flags & 2 != 0); // VMA has PROT_WRITE
```

Since the VMA no longer has `PROT_WRITE`, `is_cow_write` was false. The code
fell through to `update_page_flags(aligned_vaddr, prot_flags)` — which
re-applied the same PROT_READ flags. The CPU re-tried the write, faulted
again, and looped forever. The test timed out at 30 seconds.

**Fix:** Before the fallthrough, detect permission violations and deliver
SIGSEGV:

```rust
if reason.contains(CAUSED_BY_WRITE) && (prot_flags & 2 == 0) {
    drop(vm);
    drop(vm_ref);
    current.send_signal(SIGSEGV);
    return;
}
```

### Bug B: Access to unmapped page (vm.munmap_partial)

After `munmap()` removes a page, accessing it triggers a page fault with no
VMA. The handler called `emit_crash_and_exit(SIGSEGV, ...)` which
unconditionally killed the process via `Process::exit_by_signal()` — bypassing
any installed SIGSEGV handler.

**Fix:** Replace `emit_crash_and_exit` with `send_signal(SIGSEGV)` + return.
The interrupt return path (`x64_check_signal_on_irq_return`) delivers the
signal to the handler if one is installed. If no handler exists, the default
SIGSEGV action terminates the process.

The same fix was applied to null-pointer faults and invalid-address faults.

### Why this matters for apk

These two fixes are the only XFAIL items that were assessed as **blockers for
Alpine's apk**. Without SIGSEGV delivery, any page fault in apk's code path
(guard pages, mprotect'd regions, use-after-unmap) would either hang the
process or kill it silently instead of allowing crash recovery.

## Fix 3: mremap(2) implementation

### Motivation

musl's `realloc()` calls `mremap(MREMAP_MAYMOVE)` to grow large allocations
in-place (avoiding a `malloc` + `memcpy` + `free` round-trip). Without
`mremap`, musl falls back to the slow path. For apk processing multi-megabyte
APKINDEX files, this matters.

### Implementation

New file: `kernel/syscalls/mremap.rs` (~180 lines). Supports:

- **Shrink:** `remove_vma_range()` + unmap excess pages + TLB flush
- **Same size:** no-op, return old address
- **Grow in-place:** check if virtual space after VMA is free → `extend_vma()`
- **Grow with move** (MREMAP_MAYMOVE): allocate new VA range, move page
  mappings from old to new, remove old VMA, single remote TLB flush

Key design decisions:
- Only anonymous mappings for now (file-backed mremap deferred)
- `MREMAP_FIXED` and `MREMAP_DONTUNMAP` return `EINVAL` (not needed for musl)
- In-place grow extends the existing VMA (`extend_vma()`) rather than adding
  a new adjacent VMA — this is critical so that a subsequent shrink can find
  the single VMA covering the full range
- Huge page handling: split 2MB pages before moving individual 4KB PTEs
- Page refcounts are untouched during move (same physical page, new virtual address)

The contract test `vm.mremap_grow` validates: mmap 1 page → write sentinel →
mremap grow to 2 pages → verify sentinel survived → verify new page is
zero-filled → mremap shrink → verify sentinel again.

### Wiring

- x86_64: syscall 25, arm64: syscall 216
- `Vm::extend_vma(start, additional)` added to `kernel/mm/vm.rs`

## XFAIL audit for Alpine apk

Not everything was fixed — the remaining 6 DIVERGEs and 8 XFAILs were
audited for whether they'd block Alpine's `apk` package manager:

| Issue | Blocks apk? | Why |
|-------|-------------|-----|
| ASLR (2 tests) | No | Security, not correctness |
| getrusage zeros | No | apk doesn't check CPU time |
| uid=0 always | No | apk runs as root |
| SO_RCVBUF size | No | Performance only |
| setitimer precision | No | apk doesn't use timers |
| epoll oneshot | No | apk is synchronous |
| sigaltstack stub | No | Safety net only |
| mremap ENOSYS | **Fixed** | Now implemented |
| SIGSEGV delivery | **Fixed** | Now implemented |

## apk.static runs on Kevlar

With the fixes in place, Alpine's `apk.static` (statically linked, musl)
runs correctly:

```
$ apk.static --version
apk-tools 2.14.6, compiled for x86_64.

$ apk.static --help
usage: apk [<OPTIONS>...] COMMAND [<ARGUMENTS>...]
...
This apk has coffee making abilities.
```

## Remaining blocker: ext2 + statx path resolution

The next blocker for `apk --root /mnt` is a VFS path resolution bug. When
ext2 is mounted at `/mnt/`, C test binaries (compiled with older musl, using
`stat`/`fstatat`) can access files: `stat("/mnt/bin/busybox")` succeeds. But
BusyBox and apk.static (Alpine musl, likely using `statx`) cannot:
`test -f /mnt/bin/busybox` returns "No such file or directory."

The ext2 mount itself works — the superblock is read, blocks and inodes are
enumerated. The bug is specifically in cross-filesystem path traversal from
initramfs (tmpfs) into ext2 when using the `statx` syscall path. This is the
next debugging target.

## Test results

| Suite | Before | After |
|-------|--------|-------|
| **Contracts** | 100 PASS / 10 XFAIL / 10 DIVERGE / 1 FAIL | **104 PASS** / 8 XFAIL / 6 DIVERGE / **0 FAIL** |
| **Busybox** | 101/101 | 101/101 |
| **systemd-v3** | 25/25 | 25/25 |

## Files changed

| File | Change |
|------|--------|
| `kernel/syscalls/recvmsg.rs` | UnixStream downcast before UnixSocket |
| `kernel/mm/page_fault.rs` | SIGSEGV delivery via send_signal (3 sites) |
| `kernel/syscalls/mremap.rs` | New: mremap(2) implementation |
| `kernel/mm/vm.rs` | New: `extend_vma()` method |
| `kernel/syscalls/mod.rs` | Dispatch + constants for SYS_MREMAP |
| `testing/contracts/vm/mremap_grow.c` | New contract test |
| `testing/contracts/known-divergences.json` | +5 XFAIL, -4 stale entries |
| `testing/test_apk_update.sh` | Rewritten for apk.static --root (no chroot) |
| `tools/build-initramfs.py` | Fix resolv.conf to use QEMU DNS (10.0.2.3) |
| `Makefile` | Updated run-alpine, test-alpine targets |
