# The Ringkernel Architecture

## Overview

Kevlar uses a **ringkernel** architecture: a single-address-space kernel with
concentric trust zones enforced by Rust's type system, crate visibility, and
panic containment at ring boundaries. It combines the performance of a monolithic
kernel with the fault isolation of a microkernel — without IPC overhead.

```
    ┌─────────────────────────────────────────────────────────┐
    │  Ring 2: Services  (safe Rust, panic-contained)         │
    │  ┌──────┐ ┌──────┐ ┌──────┐ ┌────────┐ ┌───────────┐  │
    │  │ tmpfs│ │procfs│ │ext4fs│ │smoltcp │ │virtio_net │  │
    │  └──┬───┘ └──┬───┘ └──┬───┘ └───┬────┘ └─────┬─────┘  │
    │     │        │        │         │             │         │
    │  ═══╪════════╪════════╪═════════╪═════════════╪═════    │
    │     │   catch_unwind boundary (panic containment)       │
    │  ═══╪════════╪════════╪═════════╪═════════════╪═════    │
    │                                                         │
    │  Ring 1: Core  (safe Rust, trusted)                     │
    │  ┌────────┐ ┌──────────┐ ┌─────┐ ┌───────┐ ┌──────┐   │
    │  │  VFS   │ │scheduler │ │ VM  │ │signals│ │procmgr│  │
    │  └───┬────┘ └────┬─────┘ └──┬──┘ └───┬───┘ └──┬───┘   │
    │      │           │          │        │        │         │
    │  ════╪═══════════╪══════════╪════════╪════════╪═══════  │
    │      │     safe API boundary (type-enforced)            │
    │  ════╪═══════════╪══════════╪════════╪════════╪═══════  │
    │                                                         │
    │  Ring 0: Platform  (unsafe Rust, minimal TCB)           │
    │  ┌──────┐ ┌──────┐ ┌────────┐ ┌─────┐ ┌──────────┐    │
    │  │paging│ │ctxsw │ │usercopy│ │ IRQ │ │ boot/HW  │    │
    │  └──────┘ └──────┘ └────────┘ └─────┘ └──────────┘    │
    └─────────────────────────────────────────────────────────┘
```

## Design Principles

### 1. Unsafe code is confined to Ring 0

Only the `kevlar_platform` crate may contain `unsafe` blocks. All other crates
use `#![forbid(unsafe_code)]`. The platform crate exposes safe APIs that
encapsulate all hardware interaction, page table manipulation, context switching,
and user-kernel memory copying.

**Target: <10% of kernel code is unsafe** (Asterinas achieves 14%; we aim lower
by making the platform layer thinner).

### 2. Panic containment at ring boundaries

Unlike Asterinas (where any panic kills the kernel) or microkernels (where
fault isolation requires separate address spaces), Kevlar catches panics at
ring boundaries using `catch_unwind`:

- **Ring 2 → Ring 1:** A panicking service (filesystem, driver, network stack)
  has its panic caught by the Core. The Core logs the failure, drops the
  service's state, and can restart it. Other services continue running.

- **Ring 1 → Ring 0:** A panicking Core module is caught by the Platform.
  This is a more serious failure but can still be logged and potentially
  recovered (e.g., reset the scheduler, reinitialize a subsystem).

This requires that code at ring boundaries is **unwind-safe**: it must not
hold locks or leave shared state in an inconsistent state when unwinding.
Ring boundaries use message-passing patterns (submit work, receive results)
rather than shared mutable state.

### 3. Capability-based access control

Services don't get raw references to Core APIs. Instead, they receive
**capability tokens** — unforgeable typed handles that grant specific
permissions. A filesystem service receives a `PageAllocCap` (can allocate
pages), `BlockDevCap` (can read/write blocks), and `InodeCap` (can register
inodes) — but never a `PageTableCap` (can't manipulate page tables directly).

Capabilities are zero-cost wrappers (newtypes around references) that are
erased at compile time. No runtime overhead.

```rust
/// A capability to allocate physical page frames.
/// Only the Core can mint these; Services receive them at registration.
pub struct PageAllocCap<'a> {
    allocator: &'a dyn FrameAllocator,
}

impl PageAllocCap<'_> {
    pub fn alloc(&self, count: usize) -> Result<OwnedFrames> { ... }
    pub fn alloc_zeroed(&self, count: usize) -> Result<OwnedFrames> { ... }
    // No dealloc — frames are freed on drop via RAII
}
```

### 4. No IPC — direct function calls

All ring crossings are direct Rust function calls in a shared address space.
There is no serialization, no message queues, no context switches for
inter-ring communication. This is why the ringkernel matches monolithic
kernel performance despite having isolation boundaries.

The key insight: Rust's ownership system provides the same invariants that
IPC provides (no shared mutable state, clear ownership transfer) without
the performance cost.

## Comparison with Existing Approaches

| Property              | Monolithic | Microkernel | Framekernel (Asterinas) | **Ringkernel (Kevlar)** |
|-----------------------|------------|-------------|-------------------------|--------------------------|
| Address space         | Single     | Multiple    | Single                  | **Single**               |
| Isolation mechanism   | None       | HW (MMU)    | Type system (2 tiers)   | **Type system (3 tiers)**|
| Fault containment     | None       | Process     | None                    | **catch_unwind at rings**|
| IPC overhead          | N/A        | High        | None                    | **None**                 |
| Driver restart        | No         | Yes         | No                      | **Yes (Ring 2)**         |
| TCB (% of code)       | 100%       | ~5%         | ~14%                    | **<10% target**          |
| Performance vs Linux  | Baseline   | -10-30%     | ~parity                 | **~parity target**       |
| Panic behavior        | Kernel crash | Service crash | Kernel crash          | **Service restart**      |

## Ring 0: The Platform (`kevlar_platform`)

The Platform is the only crate that touches hardware. It provides safe APIs
for everything above it. Target size: ~3,000 lines of unsafe code.

### Subsystems

#### Memory: Frames and Page Tables

```rust
/// A physical page frame with exclusive ownership.
/// Cannot be aliased. Contents are zeroed on allocation.
/// The frame is freed when dropped.
pub struct OwnedFrame { /* private */ }

impl OwnedFrame {
    /// Read bytes from this frame. Returns a copy, never a reference.
    pub fn read(&self, offset: usize, buf: &mut [u8]) -> Result<()>;

    /// Write bytes to this frame.
    pub fn write(&self, offset: usize, data: &[u8]) -> Result<()>;

    /// Physical address (for page table mapping). Cannot be dereferenced
    /// in safe code — only the Platform can convert PAddr to pointers.
    pub fn paddr(&self) -> PAddr;
}

/// Page table handle. Can map/unmap user pages.
/// Cannot be constructed in safe code — only Platform creates these.
pub struct PageTable { /* private */ }

impl PageTable {
    pub fn map_user_page(&mut self, vaddr: UserVAddr, frame: &OwnedFrame, perms: PagePerms);
    pub fn unmap_user_page(&mut self, vaddr: UserVAddr) -> Option<OwnedFrame>;
    pub fn switch(&self);  // Make this the active page table
}
```

Key safety invariant: `OwnedFrame` never hands out `&[u8]` or `&mut [u8]`
to its backing memory. All access is through copy operations (`read`/`write`).
This prevents safe code from holding references into physical memory that
could be invalidated by page table changes.

#### User-Kernel Boundary

```rust
/// A validated user-space virtual address.
/// Can only be constructed by the Platform after bounds checking.
pub struct UserPtr<T> { /* private */ }

impl<T: Pod> UserPtr<T> {
    /// Copy a value from user space. Returns owned data, never a reference.
    pub fn read(&self) -> Result<T>;

    /// Copy a value to user space.
    pub fn write(&self, value: &T) -> Result<()>;
}

/// Copy a byte slice from user space.
pub fn copy_from_user(src: UserVAddr, dst: &mut [u8]) -> Result<()>;

/// Copy a byte slice to user space.
pub fn copy_to_user(dst: UserVAddr, src: &[u8]) -> Result<()>;
```

The `Pod` trait (Plain Old Data) ensures only `Copy` + `repr(C)` types cross
the user-kernel boundary. No references, no pointers, no types with drop
glue. This prevents TOCTOU and use-after-free across the boundary.

#### Context Switch and Tasks

```rust
/// An opaque kernel task. The Platform manages the kernel stack,
/// saved registers, and FPU state internally.
pub struct Task { /* private */ }

impl Task {
    pub fn new_user(entry: UserVAddr, stack: UserVAddr, page_table: &PageTable) -> Result<Task>;
    pub fn fork(parent_regs: &SavedRegs) -> Result<(Task, SavedRegs)>;
}

/// Yield the current task. The scheduler (Ring 1) decides what runs next.
pub fn switch_to(next: &Task);

/// Enter user mode. Returns when a syscall, interrupt, or fault occurs.
pub fn enter_usermode(task: &mut Task) -> UserEvent;

pub enum UserEvent {
    Syscall { number: usize, args: [usize; 6] },
    PageFault { addr: UserVAddr, write: bool },
    Interrupt { vector: u8 },
    Signal,
}
```

The `enter_usermode` → `UserEvent` pattern is the key innovation for the
user-kernel boundary. Instead of the Platform calling up into the Core
(which requires the Core to trust the Platform's call conventions), the
Platform returns a value describing what happened. The Core then dispatches
the event in safe Rust.

#### Interrupts and Timers

```rust
/// Register a handler for an IRQ line. The handler runs in interrupt
/// context (no sleeping, no allocations).
pub fn register_irq(irq: u32, handler: fn(irq: u32));

/// Acknowledge an interrupt (send EOI).
pub fn ack_irq(irq: u32);

/// Register a periodic timer callback.
pub fn register_timer(hz: u32, handler: fn());

/// Read a monotonic nanosecond timestamp.
pub fn monotonic_nanos() -> u64;
```

### What's NOT in Ring 0

The Platform does NOT contain:
- Syscall dispatch logic (Ring 1)
- Process/thread lifecycle management (Ring 1)
- Scheduling policy (Ring 1)
- VFS or any filesystem (Ring 1/2)
- Network stack (Ring 2)
- Device drivers (Ring 2)
- Signal delivery logic (Ring 1)

## Ring 1: The Core (`kevlar_core`)

The Core implements OS policies using only safe Rust and Platform APIs.
It is trusted (a Core panic is serious) but contains no unsafe code.

`#![forbid(unsafe_code)]`

### Subsystems

- **Process Manager** — process lifecycle, PID allocation, parent/child tracking
- **Scheduler** — round-robin (upgradeable to CFS), task selection, run queues
- **Virtual Memory Manager** — VMA tracking, demand paging policy, mmap/munmap
- **VFS Layer** — path resolution, mount table, inode/dentry cache, file descriptor table
- **Signal Manager** — signal delivery, handler dispatch, default actions
- **Syscall Dispatcher** — decodes `UserEvent::Syscall` into typed calls

### Panic Containment for Services

```rust
/// Call a Ring 2 service, catching panics.
/// If the service panics, return Err and log the failure.
pub fn call_service<F, R>(service_name: &str, f: F) -> Result<R>
where
    F: FnOnce() -> Result<R> + UnwindSafe,
{
    match std::panic::catch_unwind(f) {
        Ok(result) => result,
        Err(panic_info) => {
            log::error!("service '{}' panicked: {:?}", service_name, panic_info);
            Err(Errno::EIO.into())
        }
    }
}
```

The Core wraps every call into Ring 2 with `catch_unwind`. A filesystem
that panics during `read()` returns `EIO` to the caller instead of taking
down the kernel.

## Ring 2: Services (`kevlar_svc_*`)

Services are individual crates, each with `#![forbid(unsafe_code)]`.
They implement specific functionality through traits defined by the Core.

```rust
// In kevlar_core:
pub trait Filesystem: Send + Sync + UnwindSafe {
    fn lookup(&self, parent: InodeRef, name: &str) -> Result<InodeRef>;
    fn read(&self, inode: InodeRef, offset: usize, buf: &mut [u8]) -> Result<usize>;
    fn write(&self, inode: InodeRef, offset: usize, data: &[u8]) -> Result<usize>;
    // ...
}

// In kevlar_svc_tmpfs:
#![forbid(unsafe_code)]

pub struct TmpFs { /* ... */ }

impl Filesystem for TmpFs {
    fn lookup(&self, parent: InodeRef, name: &str) -> Result<InodeRef> {
        // Pure safe Rust implementation
    }
    // ...
}
```

### Service Registration

```rust
// During kernel init:
let tmpfs = TmpFs::new(page_alloc_cap);
core.register_filesystem("tmpfs", Box::new(tmpfs));

let net = SmoltcpStack::new(net_cap);
core.register_network_stack(Box::new(net));
```

Services receive capability tokens at registration time that constrain
what Platform resources they can access.

## Migration Plan

The ringkernel architecture will be implemented incrementally, without
breaking the existing kernel. The plan has four phases:

### Phase 1: Extract the Platform

Move all unsafe code from `kernel/` and `runtime/` into a new
`kevlar_platform` crate. Create safe wrapper APIs. Add
`#![forbid(unsafe_code)]` to the kernel crate.

This is the critical phase — it establishes the safety boundary.

**Files to move:**
- `runtime/x64/paging.rs` → `platform/arch/x64/paging.rs`
- `runtime/x64/boot.rs` → `platform/arch/x64/boot.rs`
- `runtime/x64/interrupt.rs` → `platform/arch/x64/interrupt.rs`
- `runtime/x64/usercopy.S` → `platform/arch/x64/usercopy.S`
- `runtime/address.rs` (unsafe parts) → `platform/address.rs`
- `kernel/arch/x64/process.rs` (context switch) → `platform/arch/x64/task.rs`
- `kernel/lang_items.rs` (memcpy/memset) → `platform/mem.rs`

**Estimated effort:** ~2,000 lines moved, ~500 lines of new safe wrappers.

### Phase 2: Define Core Traits

Create trait interfaces for VFS, scheduler, process manager, and signal
delivery. The existing implementations stay in place but now implement
traits rather than being called directly.

### Phase 3: Extract Services

Move tmpfs, procfs, devfs, smoltcp, and virtio into separate service crates.
Each gets `#![forbid(unsafe_code)]`.

### Phase 4: Add Panic Containment

Add `catch_unwind` at Ring 1→Ring 2 boundaries. Implement service restart
for filesystem and driver panics.

## Provenance

This architecture is an original design for Kevlar. It was developed
with awareness of the following prior work (design concepts only, no code):

| Reference | License | What we studied | What we took |
|-----------|---------|-----------------|--------------|
| Asterinas | MPL-2.0 | Framekernel concept, OSTD API categories | The idea of crate-level unsafe confinement |
| RedLeaf   | MIT     | Language domain concept | The idea of panic-based fault containment |
| Tock OS   | MIT/Apache-2.0 | Capability-based capsule design | Capability tokens for service access control |
| Theseus   | MIT     | Intralingual OS design | Confirmation that safe-Rust-only services are viable |

The three-ring structure, `catch_unwind`-based fault containment,
`enter_usermode` → `UserEvent` return pattern, `OwnedFrame` with
copy-only access, `Pod`-constrained `UserPtr`, and capability-token
service registration are original to Kevlar.
