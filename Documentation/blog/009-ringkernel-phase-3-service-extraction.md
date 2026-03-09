# Ringkernel Phase 3: Extracting Services

**Date:** 2026-03-08

---

Phase 1 drew the line between safe and unsafe code. Phase 2 defined trait boundaries between the Core and Services. Phase 3 moves actual service implementations out of the kernel crate into standalone crates that enforce `#![forbid(unsafe_code)]` at the compiler level.

## The shared VFS crate

Before extracting any filesystem, we needed a shared vocabulary crate. Both the kernel and service crates need to agree on types like `FileLike`, `Directory`, `Stat`, `SockAddr`, and `UserBuffer` — but these can't live in the kernel crate (that would create a circular dependency) and they can't live in a service crate (wrong direction).

`libs/kevlar_vfs` is the solution. It contains:

- **VFS traits** — `FileSystem`, `Directory`, `FileLike`, `Symlink` with their full method signatures
- **VFS types** — `INode`, `DirEntry`, `PollStatus`, `OpenOptions`, `INodeNo`, `FileType`
- **Error types** — `Errno`, `Error`, `Result` (the kernel's error system, needed by all trait impls)
- **Path types** — `Path`, `PathBuf`, `Components`
- **Stat types** — `Stat`, `FileMode`, `FileSize`, permission constants
- **Socket types** — `SockAddr`, `SockAddrIn`, `SockAddrUn`, `ShutdownHow`, `RecvFromFlags`
- **User buffer types** — `UserBuffer`, `UserBufferMut`, `UserBufReader`, `UserBufWriter`

The kernel crate re-exports everything from `kevlar_vfs` through existing module paths, so `use crate::fs::inode::FileLike` continues to work throughout the kernel. No mass import changes needed.

### The orphan rule problem

Moving `SockAddr` to `kevlar_vfs` broke the `impl From<IpEndpoint> for SockAddr` that lived in the kernel — neither `SockAddr` (now in `kevlar_vfs`) nor `IpEndpoint` (in `smoltcp`) is local to the kernel crate. Rust's orphan rule forbids this.

The fix: convert the `From`/`TryFrom` impls to freestanding functions:

```rust
// Before (broken by orphan rule):
impl TryFrom<SockAddr> for IpEndpoint { ... }
impl From<IpEndpoint> for SockAddr { ... }

// After (works from any crate that depends on both):
pub fn sockaddr_to_endpoint(sockaddr: SockAddr) -> Result<IpEndpoint> { ... }
pub fn endpoint_to_sockaddr(endpoint: IpEndpoint) -> SockAddr { ... }
```

This pattern will recur as we extract more types to shared crates — the orphan rule is a real constraint in kernel decomposition.

## Extracted service crates

### `services/kevlar_tmpfs`

The tmpfs implementation was the cleanest extraction candidate. Its only dependencies are:

- `kevlar_vfs` — VFS traits and types
- `kevlar_platform` — `SpinLock` (interrupt-safe locking)
- `kevlar_utils` — `Once`, `downcast`
- `hashbrown` — `no_std` HashMap

No kernel-internal state, no scheduler coupling, no IRQ handling. The entire 300-line implementation moved unchanged, gaining `#![forbid(unsafe_code)]` — the compiler now guarantees tmpfs contains no unsafe code.

DevFS and ProcFS both internally wrap a `TmpFs` instance, so they benefit too — their backing store is now provided by an audited, `unsafe`-free service crate.

### `services/kevlar_initramfs`

The cpio newc parser was also cleanly extractable, with one wrinkle: `include_bytes!` needs the `INITRAMFS_PATH` env var set during kernel build. The solution: the parser (`InitramFs::new(&'static [u8])`) lives in the service crate, while the thin `init()` function that calls `include_bytes!` stays in the kernel.

### What we deferred

Three subsystems are too tightly coupled to kernel internals for extraction right now:

- **smoltcp network stack** — needs `SOCKET_WAIT_QUEUE` (process sleep/wake) and `INTERFACE` (packet I/O tied to IRQ handling). Extracting this requires a `WaitQueueHandle` abstraction first.
- **devfs** — populates itself with kernel-specific devices (serial TTY, PTY). Depends on process state and TTY layer.
- **procfs** — reads process state, scheduler stats, network stats. Every file is a kernel introspection point.

These will be addressed in future phases as we build the abstractions they need.

## QEMU cleanup

A recurring annoyance: `timeout` killing `make run` left QEMU processes alive with ports bound, causing "Could not set up host forwarding rule" errors on the next run. The root cause was `preexec_fn=os.setsid` in `run-qemu.py` — QEMU got its own process group and didn't receive the SIGTERM.

The fix: forward SIGTERM/SIGINT to QEMU's process group in the Python wrapper:

```python
signal.signal(signal.SIGTERM, lambda sig, _: os.killpg(p.pid, sig))
signal.signal(signal.SIGINT, lambda sig, _: os.killpg(p.pid, sig))
```

## Results

The kernel's trust boundary is now physically enforced by the crate system:

| Crate | Ring | `unsafe` policy | Lines |
|-------|------|----------------|-------|
| `kevlar_platform` | 0 | `#![allow]` | ~3,500 |
| `kevlar_kernel` | 1 | `#![deny]` + 7 exceptions | ~15,000 |
| `kevlar_vfs` | shared | `#![forbid]` | ~500 |
| `kevlar_tmpfs` | 2 | `#![forbid]` | ~300 |
| `kevlar_initramfs` | 2 | `#![forbid]` | ~280 |

BusyBox boots and runs commands identically before and after extraction — the re-export pattern ensures binary-level compatibility.

## What's next

Phase 4: panic containment. With services in their own crates, we can wrap every call from Ring 1 into Ring 2 with `catch_unwind`. A filesystem panic during `read()` will return `EIO` instead of crashing the kernel. This is where the ringkernel pays off — three phases of refactoring enable a single `catch_unwind` wrapper that gives us microkernel-grade fault isolation at monolithic kernel performance.
