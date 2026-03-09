# Ringkernel Phase 2: Core Traits and the Service Registry

**Date:** 2026-03-08

---

Kevlar's syscall layer no longer hardcodes concrete types for socket creation or scheduling. Phase 2 introduced trait interfaces at the boundaries where Phase 4 will insert `catch_unwind` for panic containment, plus a service registry that decouples the Core from service implementations.

## What changed

Phase 1 drew the line between safe and unsafe code. Phase 2 draws the line between **Core** (trusted kernel policy) and **Services** (replaceable, panic-containable implementations). The key question for every subsystem: "If this panics, should the kernel crash?" If not, it's a service and needs a trait boundary.

### NetworkStackService

The biggest change. Previously, `sys_socket()` hardcoded concrete types:

```rust
// Before: syscall dispatch knew about smoltcp internals
let socket = match (domain, socket_type, protocol) {
    (AF_UNIX, SOCK_STREAM, 0) => UnixSocket::new() as Arc<dyn FileLike>,
    (AF_INET, SOCK_DGRAM, _) => UdpSocket::new() as Arc<dyn FileLike>,
    (AF_INET, SOCK_STREAM, _) => TcpSocket::new() as Arc<dyn FileLike>,
    ...
};
```

Now it goes through a trait:

```rust
// After: syscall dispatch is network-stack-agnostic
let net = services::network_stack();
let socket = match (domain, socket_type, protocol) {
    (AF_UNIX, SOCK_STREAM, 0) => net.create_unix_socket()?,
    (AF_INET, SOCK_DGRAM, _) => net.create_udp_socket()?,
    (AF_INET, SOCK_STREAM, _) => net.create_tcp_socket()?,
    ...
};
```

The trait itself is minimal — four methods:

```rust
pub trait NetworkStackService: Send + Sync {
    fn create_tcp_socket(&self) -> Result<Arc<dyn FileLike>>;
    fn create_udp_socket(&self) -> Result<Arc<dyn FileLike>>;
    fn create_unix_socket(&self) -> Result<Arc<dyn FileLike>>;
    fn process_packets(&self);
}
```

`SmoltcpNetworkStack` implements this trait, wrapping the existing smoltcp globals. The deferred packet processing job also goes through the service registry now, so the entire network data path is behind the trait boundary.

### SchedulerPolicy

The scheduler was already well-structured — its public API (`enqueue`, `pick_next`, `remove`) mapped directly to a trait:

```rust
pub trait SchedulerPolicy: Send + Sync {
    fn enqueue(&self, pid: PId);
    fn pick_next(&self) -> Option<PId>;
    fn remove(&self, pid: PId);
}
```

The existing round-robin `Scheduler` implements this trait. No call sites changed — the methods already had the right signatures. This is a zero-cost refactor that enables future pluggable scheduling (CFS, deadline scheduling) without modifying the Core.

### ServiceRegistry

A new `kernel/services.rs` module centralizes service access:

```rust
static NETWORK_STACK: Once<Arc<dyn NetworkStackService>> = Once::new();

pub fn register_network_stack(service: Arc<dyn NetworkStackService>) {
    NETWORK_STACK.init(|| service);
}

pub fn network_stack() -> &'static Arc<dyn NetworkStackService> {
    &*NETWORK_STACK
}
```

During boot, `main.rs` registers the concrete implementation:

```rust
services::register_network_stack(Arc::new(net::SmoltcpNetworkStack));
```

This pattern will extend to filesystem services in Phase 3.

## What we didn't change (and why)

### VFS traits stay as-is

The VFS already had good trait abstractions: `FileSystem`, `Directory`, `FileLike`, `Symlink`. These are the right granularity for service boundaries. We added documentation marking them as Ring 2 boundaries but didn't restructure them — that's Phase 3 work when the filesystem implementations actually move to separate crates.

### No UnwindSafe bounds yet

Phase 4 needs service trait methods to be callable from `catch_unwind`. We considered adding `UnwindSafe` bounds to the traits now, but deferred it. The reason: implementations hold `SpinLock` internally, which isn't `UnwindSafe`. Phase 4 will use `AssertUnwindSafe` at the catch boundary instead, with the understanding that a panicking service's entire state is dropped — the poisoned lock dies with it.

### FileLike keeps socket methods

`FileLike` currently mixes file operations (`read`, `write`, `stat`) with socket operations (`bind`, `connect`, `sendto`). Splitting into `FileLike` + `SocketOps` would be cleaner, but it's a large refactor touching every socket implementation. We documented the grouping with comments and will split in Phase 3 when the network stack moves to its own crate.

### Process manager and signals stay concrete

Process lifecycle management (fork, exec, exit, wait) and signal delivery are fundamentally Core — they manipulate PID tables, process trees, and CPU register frames. A panic here means the kernel has a bug, not that a service misbehaved. No trait extraction needed.

## Subsystem classification

| Subsystem | Ring | Trait boundary | Panic behavior |
|-----------|------|---------------|----------------|
| Platform (paging, ctx switch, IRQ) | 0 | `kevlar_platform` crate | Kernel halt |
| Process manager | 1 (Core) | Concrete `Process` struct | Kernel panic |
| Scheduler | 1 (Core) | `SchedulerPolicy` trait | Kernel panic |
| Signal delivery | 1 (Core) | Concrete `SignalDelivery` | Kernel panic |
| VFS path resolution | 1 (Core) | Concrete `RootFs` | Kernel panic |
| Filesystem impls | 2 (Service) | `FileSystem` + `Directory` + `FileLike` | **EIO** (Phase 4) |
| Network stack | 2 (Service) | `NetworkStackService` | **EIO** (Phase 4) |
| Device drivers | 2 (Service) | `EthernetDriver` (kevlar_api) | **EIO** (Phase 4) |

## What's next

**Phase 3: Extract services.** Move tmpfs, procfs, devfs, smoltcp, and virtio into separate crates, each with `#![forbid(unsafe_code)]`. The trait boundaries from Phase 2 are the extraction seams.

**Phase 4: Panic containment.** Wrap Ring 2 calls with `catch_unwind`. A panicking filesystem returns `EIO`. A panicking network stack drops connections gracefully. Service restart becomes possible.
