# Blog 129: Phase 3 complete — xattr, fdatasync, build tools 19/19 PASS

**Date:** 2026-03-29
**Milestone:** M10 Alpine Linux — Phase 3 (Build & Package Ecosystem)

## Summary

Phase 3 delivers the build ecosystem needed for Alpine package development:

1. **12 xattr syscalls** — full extended attribute support for fakeroot/abuild
2. **O_TMPFILE + linkat AT_EMPTY_PATH** — atomic file creation pattern
3. **setgroups/getgroups** — per-process supplementary group storage
4. **fdatasync** — missing syscall that broke SQLite entirely
5. **19/19 integration tests** — git, sqlite, perl, gcc/make, xattr all pass

## Extended attributes (xattr)

Implemented all 12 xattr syscalls:
- `setxattr` / `lsetxattr` / `fsetxattr`
- `getxattr` / `lgetxattr` / `fgetxattr`
- `listxattr` / `llistxattr` / `flistxattr`
- `removexattr` / `lremovexattr` / `fremovexattr`

Storage: global in-memory `HashMap<(dev_id, inode_no), HashMap<String, Vec<u8>>>`.
Works across all filesystem types (tmpfs, initramfs, ext4). Supports
`XATTR_CREATE` / `XATTR_REPLACE` flags, size queries, NUL-separated name lists.

Needed by: fakeroot (capability storage), abuild (Alpine package builder),
git (sparse-checkout metadata), rsync (attribute preservation).

## O_TMPFILE + linkat AT_EMPTY_PATH

`openat(O_TMPFILE)` now creates an anonymous temporary file in `/tmp` (tmpfs)
instead of returning ENOSYS. The file isn't linked to any directory entry
and is cleaned up when the fd is closed.

`linkat(fd, "", ..., AT_EMPTY_PATH)` resolves the fd's inode directly
and links it to the destination path, enabling the atomic file creation
pattern: `open(O_TMPFILE) → write → linkat`.

## setgroups / getgroups

Replaced the `Ok(0)` stub with real per-process supplementary group storage:
- `groups: SpinLock<Vec<u32>>` in the Process struct
- Inherited on fork/vfork/clone
- `setgroups(size, list)` reads GID array from userspace
- `getgroups(size, list)` returns stored GIDs (size=0 returns count)

## Critical bug: fdatasync missing

### The problem

`fdatasync(2)` (syscall 75 on x86_64, 83 on ARM64) was completely
unimplemented — not even a stub. The kernel returned ENOSYS for every call.

### The impact

SQLite calls `fdatasync()` after every write to ensure durability. Without
it, every `CREATE TABLE`, `INSERT`, and `PRAGMA journal_mode=WAL` failed
with "disk I/O error (10)" — SQLITE_IOERR. This made SQLite completely
non-functional.

### The fix

Added `SYS_FDATASYNC` constants for both x86_64 (75) and ARM64 (83),
dispatched to the existing `sys_fsync()` handler. For tmpfs and initramfs,
fdatasync and fsync are equivalent (no disk to sync to).

## Integration test results: 19/19 PASS

| Package | Tests | Details |
|---------|-------|---------|
| apk update | 1/1 | HTTP package index download |
| **git** | 4/4 | install, `--version`, `init` + `commit`, `log --oneline` |
| **sqlite** | 4/4 | install, `--version`, CREATE+INSERT+SELECT, WAL journal mode |
| **perl** | 5/5 | install, `-v`, `print`, file I/O (`open`/`close`), regex capture |
| **gcc/make** | 4/4 | install build-base, `make` build, run compiled binary, shared library link+run |
| **xattr** | 1/1 | `setfattr` + `getfattr` via Alpine's attr package |

### Test infrastructure

- `testing/test_build_tools.c` — C test program following test_alpine_apk pattern
- `make test-build-tools` — Makefile target (requires `build/alpine.img`)
- 600s timeout (package downloads + compilation take time)

### What this validates

- **Dynamic linking**: perl, git, sqlite are dynamically linked against musl
- **Shared libraries**: gcc builds and links .so files correctly
- **File locking**: sqlite WAL mode uses fcntl F_SETLK/F_GETLK
- **Process management**: make spawns gcc subprocesses via fork+exec
- **Filesystem**: git creates repos, sqlite writes databases, perl does file I/O
- **Networking**: apk update downloads over HTTP
- **Extended attributes**: setfattr/getfattr roundtrip via kernel xattr table

## Phase completion status

All three phases of the Alpine compatibility roadmap are now complete:

| Phase | Scope | Status | Tests |
|-------|-------|--------|-------|
| **Phase 1** | Core POSIX gaps | Complete + hardened | 118 contract tests |
| **Phase 2** | Network services | Complete | SSH 3/3, nginx 4/4 |
| **Phase 3** | Build ecosystem | Complete | Build tools 19/19 |

**Total test coverage: 300+ tests across 10+ suites, 0 failures.**

## Files changed

| Area | Files |
|------|-------|
| xattr | `kernel/syscalls/xattr.rs` (new), `kernel/syscalls/mod.rs` |
| O_TMPFILE | `kernel/syscalls/openat.rs`, `kernel/syscalls/linkat.rs` |
| setgroups | `kernel/process/process.rs`, `kernel/syscalls/mod.rs`, `kernel/syscalls/getgroups.rs` |
| fdatasync | `kernel/syscalls/mod.rs` |
| ENODATA | `libs/kevlar_vfs/src/result.rs` |
| Integration test | `testing/test_build_tools.c`, `Makefile`, `tools/build-initramfs.py` |
