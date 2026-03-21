# M10 Alpine Linux — Current Status

**Last updated:** 2026-03-21 (post-blog-098)
**Contract tests:** 116/118 PASS, 2 XFAIL, 0 FAIL
**Benchmarks:** 32 faster, 12 OK, 0 regression vs Linux KVM (44 benchmarks)

---

## Phase completion summary

| Phase | Status | Notes |
|-------|--------|-------|
| 1: Alpine rootfs | **DONE** | Blog 062 |
| 2: Interactive login (getty) | **DONE** | Blog 062 |
| 3: OpenRC boot | **DONE** | Blog 063 |
| 4: Writable ext4 | **PARTIAL** | ext2 RW done (blog 067); ext4 extents read done; ext4 write pending |
| 5: Device management (mdev) | **PARTIAL** | /dev pre-populated at boot; mdev -s not working; mknod stub |
| 6: Networking (DHCP/DNS/APK) | **MOSTLY DONE** | APK update works over TCP/HTTP (blog 089); AF_PACKET stub |
| 7: Multi-user + security | **PARTIAL** | Blog 085 started; UID/GID partially tracked; VFS permission checks stub |
| 8: Ubuntu Server | **NOT STARTED** | debootstrap, apt, SSH on ext4 |

---

## Remaining work for M10 completion

### 1. ext4 read-write (Phase 4 — most impactful gap)

**Current state:** We have ext2 read-write (`services/kevlar_ext2`). ext4
extents (the new block addressing format) are read-supported. Write is missing.

**What's needed:**
- `write()` to existing file: allocate extents, write data pages, update inode size
- `create()`: allocate inode, add directory entry, set up extent tree
- `unlink()`: remove directory entry, free inode + extent tree
- `mkdir()` / `rmdir()` / `rename()`
- `fsync()`: flush dirty pages and metadata to virtio-blk
- Journal bypass: mount with `-o norecovery` initially; add jbd2 replay later
- MBR/GPT partition parsing for real disks

**Blocker for:** Ubuntu Server (Phase 8), real hardware boot (M10.5)

---

### 2. Contract tests: 2 remaining XFAILs

**`events.inotify_create_xfail`**

`inotify_init1()` returns a valid fd but `IN_CREATE` events are never
delivered. The watch is registered (no error) but the inotify fd never
becomes readable.

Root cause: `inotify_add_watch()` registers the watch, but our VFS
create/unlink paths don't call into the inotify subsystem to fire events.

Fix: hook `inotify_notify_create()` / `inotify_notify_delete()` into the
relevant VFS operations (`create`, `unlink`, `mkdir`, `rmdir`, `rename`).

Estimated effort: 1-2 days.

**`events.poll_basic`**

`poll()` on pipe fds blocks indefinitely (30s timeout). The test creates a
pipe, writes to the write end, then polls the read end with `POLLIN`.

Root cause: `POLL_WAIT_QUEUE` wakeup is not working correctly for pipes in
the poll path. The pipe write calls `waitq.wake_all()` but the poll wakeup
path may not be connected to the same wait queue.

Fix: audit the pipe `PollStatus` → wait queue connection in the `poll()`
syscall handler. Ensure `PipeShared.waitq` is the same queue that `poll()`
sleeps on.

Estimated effort: 1-2 days.

---

### 3. File permission enforcement (Phase 7)

**Current state:** VFS calls open/exec/stat regardless of permission bits.
All processes run as UID 0 effectively. `chown`/`chmod` are stubs.

**What's needed:**
- VFS permission check on `open()`, `execve()`, `access()`, `stat()`
- `S_ISUID` / `S_ISGID` handling in `execve()` (setuid binaries)
- `chown(path, uid, gid)` — update inode uid/gid, write to ext2/ext4
- `chmod(path, mode)` — update inode mode bits
- `setuid(uid)` / `setgid(gid)` / `setreuid()` / `setresuid()`
- `setgroups()` / `getgroups()` — supplementary group lists
- Fork inherits parent credentials

The mechanism is in place (process has `uid`/`gid` fields) but checks are
not wired into VFS operations.

Estimated effort: 1 week.

---

### 4. mknod / real device nodes (Phase 5)

**Current state:** `/dev/` is pre-populated at boot with hardcoded entries
(null, zero, random, urandom, ttyS0, console). `mknod(2)` is a stub.

**What's needed:**
- Real `mknod(path, S_IFCHR|mode, makedev(major, minor))` in tmpfs
- Device dispatch: opening a character device node routes to the correct
  driver based on major:minor
- Existing `/dev/` entries use hardcoded major:minor — wire these through
  the new dispatch table
- `mdev -s` scans `/sys/` and calls `mknod` — sysfs device entries needed

Estimated effort: 3-5 days for mknod + dispatch; 1-2 weeks for full sysfs
device tree.

---

### 5. AF_UNIX stream sockets (no longer XFAIL but verify)

Blog 094 fixed the panic in `accept4` / `unix_stream`. These now pass
contract tests. Verify that real applications using AF_UNIX work:
- D-Bus (relies heavily on AF_UNIX)
- OpenRC's service management uses UNIX sockets
- systemd on Ubuntu Server requires UNIX socket support

---

### 6. Phase 8: Ubuntu Server (NOT STARTED)

This requires Phases 4 and 7 to be complete first. Once ext4 write and
permissions work, Ubuntu Server boot requires:

- `debootstrap` to create a minimal Ubuntu rootfs on an ext4 disk image
- Boot Kevlar with `root=/dev/vda1` (ext4 root, not initramfs)
- systemd starts from ext4 (already proven in M9, but from initramfs)
- `apt update && apt install curl` — HTTPS download + dpkg file operations
- SSH server (`openssh-server`) — needs PAM stubs, pty allocation
- Docker-ready: overlayfs or similar union FS stub

Estimated effort: 3-4 weeks (dominated by ext4 write debugging).

---

## Recommended M10 completion order

1. **Fix `events.poll_basic`** (1-2 days) — easy win, contracts → 117/118
2. **Fix `events.inotify_create_xfail`** (1-2 days) — contracts → 118/118 PASS, 0 XFAIL
3. **ext4 write** (2-3 weeks) — largest remaining gap, unblocks Phase 8
4. **mknod + device dispatch** (1 week) — unblocks mdev, sysfs
5. **File permissions + chown/chmod** (1 week) — multi-user prerequisite
6. **Ubuntu Server boot** (3-4 weeks) — M10 completion milestone

**Total remaining: ~6-8 weeks to M10 complete.**

---

## What M10 completion unlocks

- **M10.5 kcompat Phase 1-3**: Module loader + storage drivers (boot from NVMe on real hardware)
- **M11**: Alpine graphical desktop (framebuffer, X11, Wayland)
- **Blog material**: Ubuntu Server running unmodified on Kevlar is a major milestone post

## Open known-divergences.json entries to clean up

The following entries in `testing/contracts/known-divergences.json` are
likely now passing (fixed in blogs 090-095) and should be removed after
verification:

- `sockets.accept4_flags` — panic fixed in blog 094
- `sockets.unix_stream` — panic fixed in blog 094
- `process.wait4_wnohang` — PID non-determinism (test artifact, may be
  accepted as permanent XFAIL or test fixed)
- `time.clock_realtime` — 1s timing artifact (test artifact)
