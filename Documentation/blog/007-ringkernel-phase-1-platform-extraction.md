# Ringkernel Phase 1: Extracting the Platform

**Date:** 2026-03-08

---

Kevlar's kernel crate now enforces `#![deny(unsafe_code)]`. All unsafe code lives in a single crate — `kevlar_platform` — and the kernel interacts with hardware exclusively through safe Rust APIs. This is Phase 1 of the ringkernel architecture: establishing the safety boundary between the Platform (Ring 0) and the rest of the kernel.

## Why this matters

In a typical Rust kernel, `unsafe` is scattered everywhere: page table manipulation, context switching, user-kernel copies, inline assembly, raw pointer casts. Every `unsafe` block is a place where Rust's safety guarantees are suspended — a potential source of memory corruption, use-after-free, or undefined behavior. Auditing safety requires reading the entire codebase.

After Phase 1, Kevlar has a strict rule: **the kernel crate contains no unsafe code** (with 7 annotated exceptions that need targeted `#[allow(unsafe_code)]`). If you want to audit Kevlar's memory safety, you read 5,346 lines of platform code instead of 17,366 lines of everything.

```
Before:  unsafe scattered across kernel/ and runtime/
         ├── kernel/arch/x64/process.rs     (context switch, TLS)
         ├── kernel/lang_items.rs           (memcpy, memset, memcmp)
         ├── kernel/mm/page_fault.rs        (raw page zeroing)
         ├── kernel/process/switch.rs       (Arc refcount manipulation)
         ├── kernel/process/elf.rs          (pointer casts for ELF parsing)
         ├── kernel/user_buffer.rs          (raw pointer reads/writes)
         ├── kernel/random.rs              (rdrand intrinsic)
         ├── kernel/fs/path.rs             (pointer cast for newtype)
         ├── kernel/fs/initramfs.rs        (unchecked UTF-8)
         ├── kernel/syscalls/futex.rs      (raw user pointer deref)
         ├── kernel/syscalls/sysinfo.rs    (raw slice creation)
         └── runtime/  (all unsafe, but mixed with safe logic)

After:   unsafe confined to platform/
         ├── platform/         5,346 lines (Ring 0, all unsafe lives here)
         └── kernel/          12,020 lines (Ring 1+, #![deny(unsafe_code)])
                              7 exceptions with #[allow(unsafe_code)]
```

## What moved

### Architecture-specific task code

The biggest move was `kernel/arch/x64/process.rs` (and its ARM64 counterpart) into `platform/x64/task.rs`. This file contains the `ArchTask` struct (kernel stack, saved registers, FPU state) and `switch_task()` — the context switch that saves one task's registers and restores another's. The associated assembly (`usermode.S` with `syscall_entry`, `kthread_entry`, `forked_child_entry`, `do_switch_thread`) moved alongside it.

The kernel re-exports these with compatibility aliases:

```rust
// kernel/arch/x64/mod.rs — thin re-export layer
pub use kevlar_platform::arch::x64_specific::ArchTask as Process;
pub use kevlar_platform::arch::x64_specific::switch_task as switch_thread;
```

### Memory intrinsics

Custom `memcpy`, `memmove`, `memset`, `memcmp`, and `bcmp` moved from `kernel/lang_items.rs` to `platform/mem.rs`. These exist because Kevlar disables SSE in kernel mode (`+soft-float`), and the compiler-builtins implementations use 128-bit loads that require SSE. The platform crate is the natural home — it's the layer that knows about hardware constraints.

### Safe wrapper APIs

The real work wasn't moving code — it was creating safe APIs that let the kernel do everything it used to do with `unsafe`, without `unsafe`:

| Module | Safe API | Replaces |
|--------|----------|----------|
| `platform/pod.rs` | `copy_as_bytes(&value)` | `slice::from_raw_parts(ptr, size)` |
| `platform/pod.rs` | `ref_from_prefix(bytes)` | `&*(ptr as *const T)` |
| `platform/pod.rs` | `read_copy_from_slice(buf, offset)` | `*(ptr.add(offset) as *const T)` |
| `platform/pod.rs` | `str_newtype_ref(s)` | `&*(s as *const str as *const Path)` |
| `platform/page_ops.rs` | `zero_page(paddr)` | `paddr.as_mut_ptr().write_bytes(0, PAGE_SIZE)` |
| `platform/page_ops.rs` | `page_as_slice_mut(paddr)` | `slice::from_raw_parts_mut(ptr, PAGE_SIZE)` |
| `platform/sync.rs` | `arc_leak_one_ref(&arc)` | `Arc::decrement_strong_count(ptr)` |
| `platform/random.rs` | `rdrand_fill(slice)` | `x86::random::rdrand_slice(slice)` |
| `platform/x64/task.rs` | `write_fsbase(value)` | `wrfsbase(value)` |

The `Pod` (Plain Old Data) trait deserves special mention. It's `unsafe trait Pod: Copy + 'static {}`, implemented only for primitives. Functions like `as_bytes` and `from_bytes` are safe to call because the trait's safety contract guarantees any bit pattern is valid. The `unsafe` is pushed to the trait implementation (in the platform), not the call site (in the kernel).

One interesting case: `str_newtype_ref` handles `Path`, which is a `#[repr(transparent)]` newtype over `str`. You can't cast `*const str` to `*const Path` because they're unsized (fat pointers). The solution is `transmute_copy::<&str, &T>(&s)` — safe at the call site, with the `unsafe` inside the platform.

## The 7 remaining exceptions

Seven `unsafe` sites remain in the kernel with `#[allow(unsafe_code)]`:

| File | What | Why it can't move |
|------|------|-------------------|
| `main.rs` | `#[unsafe(no_mangle)] fn boot_kernel` | Entry point must have a stable symbol name |
| `main.rs` | `unsafe { &mut *frame }` in syscall handler | Raw pointer from platform's callback signature |
| `lang_items.rs` | `static mut KERNEL_DUMP_BUF` + panic handler | Crash dump needs mutable static + raw pointer ops |
| `logger.rs` | `KERNEL_LOG_BUF.force_unlock()` | Break potential deadlock during panic |
| `process.rs` (x2) | `from_raw_parts_mut(pages.as_mut_ptr(), len)` | Kernel-allocated page buffers for ELF loading |

These are all either ABI requirements (`no_mangle`), panic-path code (crash dump, deadlock breaking), or places where the platform's page allocator returns raw `PAddr` that needs to become a slice. Phase 2 can potentially eliminate the last two by adding a `page_as_slice_mut` variant to the platform's page allocator API.

## The rename

As part of this work, the `runtime/` directory was renamed to `platform/` and the crate from `kevlar_runtime` to `kevlar_platform`. This is more than cosmetic — "runtime" implies support code, while "platform" communicates that this is the hardware abstraction layer and the sole trust boundary. Every `use kevlar_runtime::` across 88 `.rs` files was updated.

## Verification

Both x86_64 and ARM64 build cleanly with zero warnings. The QEMU boot test passes — BusyBox shell reaches the interactive prompt with no regressions:

```
Booting Kevlar...
initramfs: loaded 78 files and directories (2MiB)
kext: Loading virtio_net...
virtio-net: MAC address is 52:54:00:12:34:56
running init script: "/bin/sh"

BusyBox v1.31.1 built-in shell (ash)
#
```

## What's next

Phase 1 establishes the safety boundary. The remaining phases complete the ringkernel:

- **Phase 2:** Define Core traits — VFS, scheduler, process manager, and signal delivery get trait interfaces. The kernel's subsystems implement these traits rather than being called directly. This enables Phase 3's extraction.
- **Phase 3:** Extract services — tmpfs, procfs, devfs, smoltcp, and virtio move into separate crates, each with `#![forbid(unsafe_code)]`.
- **Phase 4:** Panic containment — `catch_unwind` at Ring 1 to Ring 2 boundaries. A panicking filesystem or driver returns `EIO` instead of crashing the kernel. Service restart becomes possible.

The ringkernel design document at `Documentation/architecture/ringkernel.md` has the full architectural vision.
