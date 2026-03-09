# Beating Linux: Syscall Performance in a Rust Kernel

Blog 016 ended with getpid at 200ns and stat at 24µs — respectable, but
still 60x behind Linux for path-based syscalls. Two root causes remained:
the compiler was generating unoptimized code, and every operation paid
unnecessary overhead in locks, allocations, and copies.

After this round, every core syscall benchmark beats native Linux:

| Benchmark  | Before    | After  | Linux Native | vs Linux |
|------------|-----------|--------|--------------|----------|
| getpid     | 200 ns    | 63 ns  | 97 ns        | **1.5x faster** |
| read_null  | 514 ns    | 89 ns  | 102 ns       | **1.1x faster** |
| write_null | 517 ns    | 91 ns  | 117 ns       | **1.3x faster** |
| pipe       | 82,252 ns | 290 ns | 361 ns       | **1.2x faster** |
| open_close | 20,607 ns | 510 ns | 867 ns       | **1.7x faster** |
| stat       | 23,234 ns | 262 ns | 389 ns       | **1.5x faster** |

## The 50x fix: opt-level = 2

The dev profile in `Cargo.toml` had no `opt-level` setting, defaulting to
0 — no optimization at all. Every function call was a real call, every
variable was spilled to the stack, no inlining, no constant propagation.

```toml
[profile.dev]
opt-level = 2
panic = "abort"
```

This single line improved getpid from 3,686ns to 65ns. Every other
benchmark improved 5-50x. All the careful optimization work in blog 016
was running on unoptimized code — the real floor was 50x lower than what
we measured.

We also set `debug-assertions = false` in the dev profile. Our `SpinLock`
uses `AtomicRefCell` for deadlock tracking under `cfg(debug_assertions)`,
adding an atomic store on every lock release. With debug assertions off,
every lock acquire/release got ~10ns cheaper.

## Eliminating heap allocations from syscall paths

### StackPathBuf: zero-alloc path resolution

Every `stat()`, `open()`, `access()`, and `*at()` syscall called
`resolve_path()` which heap-allocated three times: a `Vec` for reading
the path bytes, a `String` for UTF-8 validation, and a `PathBuf` for
the result.

`StackPathBuf` replaces all of this with a 256-byte stack buffer:

```rust
struct StackPathBuf {
    buf: [u8; 256],
    len: usize,
}
```

A single `read_cstr` fills the buffer directly from userspace memory.
Seven syscall handlers were converted to use it. Paths longer than 255
bytes — rare in practice — fall back to the heap path.

### Fast VFS lookup without PathComponent

The VFS `lookup_path()` method creates an `Arc<PathComponent>` for every
path component traversed — a heap allocation plus a `String` clone for
the component name. For `stat("/tmp")`: two allocations (root dir and
"tmp"), both immediately discarded.

`lookup_inode()` is a new fast path that walks the directory tree
directly, returning an `INode` enum without creating any `PathComponent`
objects. It handles the common case (no `..`, no symlinks in
intermediate components) and falls back to the full `lookup_path()` for
the rest.

For `stat("/tmp")`: zero heap allocations instead of two.

### Lock-free Directory::inode_no()

Mount point checking used to call `dir.stat()` — which acquires a
spinlock to copy out the full `Stat` struct — just to extract the inode
number. Adding an `inode_no()` method to the `Directory` trait with a
lock-free override in tmpfs eliminated this unnecessary lock.

## Pipe: from 82µs to 290ns

The pipe implementation had three compounding problems.

**No fast path**: Even when data was immediately available, every
read/write went through `sleep_signalable_until()` which enqueues the
current process on the wait queue, checks for pending signals, and
dequeues on completion. Three spinlock acquire/release cycles for
every byte transferred.

Fix: try the operation first. If it succeeds, wake waiters and return
immediately. Only enter the sleep loop when the buffer is genuinely
full (writer) or empty (reader).

**Double-buffered copies**: Writing to a pipe copied data from
userspace into a temporary kernel buffer, then from the buffer into
the ring buffer. Reading did the reverse. Two memcpy calls per
direction.

Fix: `RingBuffer::writable_contiguous()` returns a mutable slice of
the next free region. `UserBufReader::read_bytes()` copies directly
from userspace into this slice — one copy instead of two.

**Waking nobody**: `PIPE_WAIT_QUEUE.wake_all()` acquired its spinlock
on every write, even when no process was sleeping on it.

Fix: `WaitQueue::waiter_count` tracks the number of sleeping processes
with an `AtomicUsize`. `wake_all()` checks this with a relaxed load
and returns immediately when zero — skipping the spinlock entirely.

## tmpfs: lock-free stat and lighter locks

Directory `stat()` in tmpfs acquired a spinlock to copy out a `Stat`
struct that never changes after creation (mode and inode number are
set at `Dir::new()` time). Moving the `Stat` out of the locked
`DirInner` and into the `Dir` struct itself made `Dir::stat()` lock-free.

All remaining tmpfs locks were changed from `lock()` (which does
`pushfq; cli; ...; sti; popfq`) to `lock_no_irq()` (which does
nothing extra). Tmpfs is never accessed from interrupt context, so the
interrupt save/restore was pure waste — ~20ns saved per lock
acquire/release.

## Hardware-optimized memory operations

Our custom `memset` and `memcpy` (needed because the kernel runs with
SSE disabled) used manual 8-byte store loops — 512 iterations to zero
a page. Modern x86 CPUs have hardware-optimized `rep stosb`/`rep movsb`
(Enhanced REP MOVSB, ERMS) that fill and copy memory at cache-line
granularity.

```rust
// Before: 512 iterations of write_unaligned
while i + 8 <= n {
    (dest.add(i) as *mut u64).write_unaligned(word);
    i += 8;
}

// After: single hardware-optimized instruction
core::arch::asm!("rep stosb", ...);
```

`zero_page()` uses `rep stosq` specifically, zeroing 4KB in ~50 cycles
instead of ~500.

## Demand paging: the KVM tax

The one benchmark we couldn't close was `mmap_fault` — anonymous page
fault throughput. A three-way comparison revealed why:

| Benchmark  | Linux Native | Linux KVM | Kevlar KVM |
|------------|-------------|-----------|------------|
| mmap_fault | 1,047 ns    | 2,104 ns  | 3,808 ns   |

Linux-in-KVM is already **2x slower** than Linux-native for page
faults. Every newly mapped guest page triggers an EPT (Extended Page
Table) violation: the CPU exits the guest, KVM updates the host's
nested page tables, then re-enters the guest. This costs ~1,000 cycles
per page and doesn't exist on bare metal.

Against the fair baseline (Linux KVM), Kevlar is 1.8x behind — real
overhead from our bitmap allocator and simpler page table code, but not
the 4x it appeared against native Linux.

We did fix one clear waste: pages were being zeroed twice. `alloc_pages()`
zeroed the page under the allocator lock, then `handle_page_fault()`
zeroed it again. Passing `DIRTY_OK` to the allocator and zeroing once
after the lock is released saved both the redundant memset and reduced
lock hold time.

## The optimization stack

Each layer builds on the previous:

1. **opt-level=2** (50x): Let the compiler do its job.
2. **debug-assertions=false** (1.2x): Remove per-lock atomic overhead.
3. **StackPathBuf** (2-3x for path syscalls): Zero heap allocations.
4. **Fast lookup_inode** (2-3x for path syscalls): Zero PathComponent
   allocations.
5. **Pipe fast path** (280x): Skip wait queue when data is available.
6. **Lock-free tmpfs stat** (1.3x): Don't lock immutable data.
7. **lock_no_irq everywhere** (1.1x): Don't save/restore interrupts
   when not needed.
8. **rep stosb/movsb** (1.1x): Let the CPU's microcode handle bulk
   memory operations.

The lesson is familiar: measure, find the biggest bottleneck, fix it,
repeat. The profiler from blog 016 paid for itself many times over.

## What's next

The mmap_fault gap (1.8x vs Linux KVM) needs page allocator work — our
bitmap allocator is a placeholder that should be replaced with a proper
buddy allocator. The fork benchmark is disabled pending a page table
duplication bug fix. And we haven't started on the dcache (directory
entry cache) that would make repeated path lookups nearly free.

But for the core syscall path — the thing every program does thousands
of times per second — Kevlar now beats Linux. In Rust, with
`#![deny(unsafe_code)]` on the kernel crate, running in a virtual
machine.
