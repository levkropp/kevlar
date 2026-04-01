# Blog 138: Desktop syscalls — VT ioctls, SysV SHM, D-Bus sockets, and X11 14/14

**Date:** 2026-04-01
**Milestone:** M11 Alpine Graphical — Phase 4 (Desktop Infrastructure)

## Summary

Six kernel features implemented in one push to close the gap between
"X server starts" and "desktop can run." VT ioctls for terminal
management, SysV shared memory for MIT-SHM, SO_PEERCRED and
SCM_CREDENTIALS for X11/D-Bus authentication, abstract Unix sockets
for the D-Bus message bus, and virtual terminal device nodes. All
referenced from POSIX specs and FreeBSD's BSD-licensed implementations.

Result: 14/14 X11 tests pass, 67/67 Alpine smoke tests green, zero
regressions.

## What was implemented

### 1. VT ioctls (20+ ioctl handlers)

Xorg and `startx` probe the virtual terminal subsystem heavily on
startup. Without these ioctls, Xorg falls back to degraded mode or
refuses to start.

Implemented in `kernel/fs/devfs/tty.rs`:

| Ioctl | Number | What it does |
|-------|--------|-------------|
| VT_OPENQRY | 0x5600 | Find first free VT → returns 2 |
| VT_GETMODE | 0x5601 | Get VT switching mode → VT_AUTO |
| VT_SETMODE | 0x5602 | Set switching mode → accepted silently |
| VT_GETSTATE | 0x5603 | Active VT bitmask → VT1 active |
| VT_RELDISP | 0x5605 | Release VT → accepted |
| VT_ACTIVATE | 0x5606 | Switch VT → accepted (single VT) |
| VT_WAITACTIVE | 0x5607 | Wait for VT → returns immediately |
| KDGETMODE | 0x4B3B | Text/graphics mode → KD_TEXT |
| KDSETMODE | 0x4B3A | Set mode → accepted |
| KDGKBMODE | 0x4B44 | Keyboard mode → K_XLATE |
| KDSKBMODE | 0x4B45 | Set keyboard mode → accepted |
| KDGKBTYPE | 0x4B33 | Keyboard type → KB_101 |
| KDGKBLED | 0x4B64 | LED state → 0 |

Also added `/dev/tty0` through `/dev/tty7` as device nodes, all
aliased to the serial console for now. This satisfies programs that
open `/dev/tty1` for VT operations.

### 2. SysV shared memory (4 syscalls)

X11's MIT-SHM extension uses SysV shared memory for zero-copy pixmap
transfer between client and server. Without it, every pixel goes
through the Unix socket — functional but slow.

Implemented in `kernel/syscalls/shm.rs`:

- **shmget** (syscall 29) — create or find shared memory segment.
  Allocates pre-zeroed physical pages, keyed by integer ID.
- **shmat** (syscall 30) — attach segment to process address space.
  Maps the segment's physical pages into a new VMA with refcount
  tracking.
- **shmdt** (syscall 67) — detach segment from process.
- **shmctl** (syscall 31) — control operations. Supports IPC_RMID
  (deferred removal) and IPC_STAT (query segment info).

Segments are backed by real physical pages shared across process
address spaces — not just stubs. The page refcount system handles
cleanup when all attachments are released.

Reference: FreeBSD `kern/sysv_shm.c` (BSD-2-Clause), POSIX IPC spec.

### 3. SO_PEERCRED and SCM_CREDENTIALS

X11 and D-Bus authenticate clients by checking their credentials
(pid, uid, gid) via the Unix socket.

**SO_PEERCRED** (getsockopt) — returns a `struct ucred` with the
peer's pid, uid, and gid. Added to `kernel/syscalls/getsockopt.rs`.

**SCM_CREDENTIALS** (recvmsg ancillary data) — automatically appended
to Unix socket receives when the control buffer has space. The kernel
fills in the sender's credentials. Added to
`kernel/syscalls/recvmsg.rs`.

Reference: FreeBSD `kern/uipc_usrreq.c` (BSD-2-Clause), credentials(7).

### 4. Abstract Unix socket namespace

D-Bus uses abstract Unix sockets where the address starts with `\0`
instead of a filesystem path. These sockets exist only in kernel
memory — no filesystem entry is created.

Modified `sockaddr_un_path()` in `kernel/net/unix_socket.rs` to
detect the leading `\0` byte and extract the abstract name. The
existing listener registry (a `Vec<(String, Arc<UnixListener>)>`)
handles abstract names transparently — they're just strings without
the `\0` prefix.

Reference: Linux unix(7) abstract namespace, FreeBSD local(4).

### 5. Font cache pre-generation

The Alpine X11 disk image build (`tools/build-alpine-xorg.py`) now
creates a first-boot font setup script and pre-generates minimal
`fonts.dir` files for the installed PCF bitmap fonts. This ensures
xterm can find fonts on first boot without needing to run
`mkfontscale` inside Kevlar.

### 6. What this enables

With all six features in place:

- **startx** can probe VTs, switch to graphics mode, and manage
  the display
- **X11 clients** authenticate via SCM_CREDENTIALS on Unix sockets
- **MIT-SHM** provides zero-copy pixmap transfer for responsive UI
- **D-Bus** can create its message bus on abstract sockets
- **XFCE** and other desktops have the syscall foundation they need

## Architecture note: no GPL, no FFI

Every feature here is a Rust implementation of a documented POSIX or
Linux interface. The kernel-userspace boundary is syscalls — integers
in registers. Alpine's musl-linked binaries don't know or care that
the kernel is Rust. We referenced:

- **POSIX IPC specification** (public standard) for SysV SHM semantics
- **FreeBSD kernel** (BSD-2-Clause) for VT and credential implementation patterns
- **Linux man-pages** (factual interface descriptions) for ioctl numbers and struct layouts
- **musl libc** (MIT) for understanding the userspace contract

No GPL code was referenced or linked. No C-to-Rust FFI bridges are
needed — the compatibility layer IS the syscall implementation.

## Verified

- **14/14 X11 integration tests** — device files, binaries, config, Xorg startup, xdpyinfo
- **67/67 Alpine smoke tests** — no regressions from new syscalls
- **159/159 contract tests** — ABI compatibility preserved
- All new code compiles cleanly under `#![deny(unsafe_code)]`
