# The Ringkernel Architecture

## Overview

Kevlar uses a **ringkernel** architecture: a single-address-space kernel with
concentric trust zones enforced by Rust's type system, crate visibility, and
panic containment at ring boundaries. It combines the performance of a monolithic
kernel with the fault isolation of a microkernel — without IPC overhead.

```
    ┌─────────────────────────────────────────────────────────┐
    │  Ring 2: Services  (safe Rust, panic-contained)         │
    │  ┌──────┐ ┌──────┐ ┌─────┐ ┌────────┐ ┌───────────┐   │
    │  │ tmpfs│ │procfs│ │ ext2│ │smoltcp │ │virtio_net │   │
    │  └──┬───┘ └──┬───┘ └──┬──┘ └───┬────┘ └─────┬─────┘   │
    │     │        │        │        │             │          │
    │  ═══╪════════╪════════╪════════╪═════════════╪═════     │
    │     │   catch_unwind boundary (panic containment)       │
    │  ═══╪════════╪════════╪════════╪═════════════╪═════     │
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
    │  │paging│ │ctxsw │ │usercopy│ │ SMP │ │ boot/HW  │    │
    │  └──────┘ └──────┘ └────────┘ └─────┘ └──────────┘    │
    └─────────────────────────────────────────────────────────┘
```

## Design Principles

### 1. Unsafe code is confined to Ring 0

Only the `kevlar_platform` crate may contain `unsafe` blocks. The kernel crate
enforces `#![deny(unsafe_code)]` (with 7 annotated exceptions). All service crates
use `#![forbid(unsafe_code)]`. The platform crate exposes safe APIs that encapsulate
all hardware interaction, page table manipulation, context switching, and user-kernel
memory copying.

**Target: <10% of kernel code is unsafe.** The platform layer is kept thin so
the unsafe surface area stays small and auditable.

### 2. Panic containment at ring boundaries

Unlike monolithic kernels (where any panic kills the system) or microkernels (where
fault isolation requires separate address spaces and IPC), Kevlar catches panics at
ring boundaries using `catch_unwind`:

- **Ring 2 → Ring 1:** A panicking service (filesystem, driver, network stack)
  has its panic caught by the Core. The Core logs the failure and returns `EIO`
  to the caller. Other services continue running.

- **Ring 1 → Ring 0:** A panicking Core module is caught by the Platform.
  This is a more serious failure but can still be logged and potentially recovered.

This requires `panic = "unwind"` mode (Fortress and Balanced profiles). Performance
and Ludicrous profiles use `panic = "abort"` and skip `catch_unwind` for speed.

```rust
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

### 3. Capability-based access control

Services receive **capability tokens** — unforgeable typed handles that grant specific
permissions. A filesystem service receives a `PageAllocCap` (can allocate pages) and
`BlockDevCap` (can read/write blocks) — but never a `PageTableCap`.

The token implementation varies by safety profile:
- **Fortress**: Runtime-validated nonce (unforgeable at runtime).
- **Balanced**: Zero-cost newtype (type system proves authorization at compile time).
- **Performance/Ludicrous**: Compiled away entirely.

```rust
pub struct Cap<T> {
    nonce: u64,          // Fortress: validated at ring boundary
    _marker: PhantomData<T>,
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

| Property              | Monolithic | Microkernel | Framekernel | **Ringkernel (Kevlar)** |
|-----------------------|------------|-------------|-------------|--------------------------|
| Address space         | Single     | Multiple    | Single      | **Single**               |
| Isolation mechanism   | None       | HW (MMU)    | Type system (2 tiers) | **Type system (3 tiers)**|
| Fault containment     | None       | Process     | None        | **catch\_unwind at rings**|
| IPC overhead          | N/A        | High        | None        | **None**                 |
| Driver restart        | No         | Yes         | No          | **Yes (Ring 2)**         |
| TCB (% of code)       | 100%       | ~5%         | ~10-15%     | **<10% target**          |
| Performance vs Linux  | Baseline   | -10-30%     | ~parity     | **~parity or faster**    |
| Panic behavior        | Kernel crash | Service crash | Kernel crash | **Service restart**   |

## Ring 0: The Platform (`kevlar_platform`)

The Platform is the only crate that touches hardware. It provides safe APIs
for everything above it.

### Key Safe APIs

```rust
// Physical page frames with exclusive ownership
pub struct OwnedFrame { /* private */ }
impl OwnedFrame {
    pub fn read(&self, offset: usize, buf: &mut [u8]) -> Result<()>;
    pub fn write(&self, offset: usize, data: &[u8]) -> Result<()>;
    pub fn paddr(&self) -> PAddr;
}

// Validated user-space address (Pod = Copy + repr(C))
pub struct UserPtr<T: Pod> { /* private */ }
impl<T: Pod> UserPtr<T> {
    pub fn read(&self) -> Result<T>;
    pub fn write(&self, value: &T) -> Result<()>;
}

// Opaque kernel task
pub struct Task { /* private */ }

// Three lock variants
pub struct SpinLock<T> { /* ... */ }
impl<T> SpinLock<T> {
    pub fn lock(&self) -> SpinLockGuard<T>;           // cli/sti
    pub fn lock_no_irq(&self) -> SpinLockGuardNoIrq<T>; // no cli/sti
    pub fn lock_preempt(&self) -> SpinLockGuardPreempt<T>; // IF=1, preempt disabled
}
```

See [Platform / HAL](hal.md) for the full details including SMP boot, TLB shootdown,
usercopy, and the vDSO.

## Ring 1: The Core (`kernel/`)

The Core implements OS policies using only safe Rust and Platform APIs.
It is trusted (a Core panic is serious) but contains no unsafe code.

`#![deny(unsafe_code)]`

### Subsystems

- **Process Manager** — lifecycle, PID allocation, parent/child, thread groups, cgroups, namespaces
- **Scheduler** — per-CPU round-robin with work stealing (up to 8 CPUs)
- **Virtual Memory** — VMA tracking, demand paging, CoW, transparent huge pages
- **VFS Layer** — path resolution, mount table, inode/dentry cache, fd table
- **Signal Manager** — delivery, handler dispatch, lock-free mask, signalfd
- **Syscall Dispatcher** — 141 syscall modules, 121+ dispatch entries

## Ring 2: Services

Services are individual crates, each with `#![forbid(unsafe_code)]`. They implement
functionality through traits defined in `libs/kevlar_vfs`:

```rust
// In libs/kevlar_vfs:
pub trait FileSystem: Send + Sync {
    fn root_dir(&self) -> Result<Arc<dyn Directory>>;
}

// In services/kevlar_ext2:
#![forbid(unsafe_code)]

pub struct Ext2Fs { /* ... */ }
impl FileSystem for Ext2Fs {
    fn root_dir(&self) -> Result<Arc<dyn Directory>> {
        // Pure safe Rust, reads from block device
    }
}
```

Current service crates:
- `services/kevlar_tmpfs` — in-memory read-write filesystem
- `services/kevlar_initramfs` — cpio newc archive parser (boot-time)
- `services/kevlar_ext2` — ext2/3/4 read-write filesystem on VirtIO block

Services that are not yet extracted (too tightly coupled to kernel internals):
smoltcp networking, procfs, sysfs, devfs.

## Implementation Status

All four phases of the ringkernel implementation are complete:

### Phase 1: Extract the Platform ✓

All unsafe code moved from `kernel/` into `kevlar_platform`. Safe wrapper APIs
created. The kernel crate enforces `#![deny(unsafe_code)]`.

### Phase 2: Define Core Traits ✓

Service traits defined at Ring 2 boundaries: `NetworkStackService`,
`SchedulerPolicy`, `FileSystem`, `Directory`, `FileLike`, `Symlink`.
`ServiceRegistry` provides centralized access to Ring 2 services.

### Phase 3: Extract Services ✓

Shared VFS types extracted to `libs/kevlar_vfs` (`#![forbid(unsafe_code)]`).
Three service crates created: `kevlar_tmpfs`, `kevlar_initramfs`, `kevlar_ext2`.

### Phase 4: Safety Profiles ✓

Four compile-time safety profiles (Fortress, Balanced, Performance, Ludicrous)
control ring count, catch\_unwind, frame access, and capability checking.
See [Safety Profiles](safety-profiles.md).
