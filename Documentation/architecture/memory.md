# Memory Management

## Virtual Address Space Layout (x86\_64)

```
0x0000_0000_0000 – 0x0000_7fff_ffff   User space (128 GB)
0x1000_0000_0000                       vDSO (single 4 KB page)
0xffff_8000_0000_0000+                 Kernel (higher half)
```

The kernel uses a direct-mapped physical memory region in the higher half, plus a
separate VALLOC area for kernel virtual address allocations.

## VMAs

User virtual memory is tracked as a list of `VmArea` structs in `kernel/mm/vm.rs`.
Each VMA records:

- `start` and `end` virtual addresses (page-aligned)
- `prot`: `MMapProt` flags (READ, WRITE, EXEC)
- `flags`: `MMapFlags` (SHARED, PRIVATE, ANONYMOUS, FIXED, etc.)
- The backing object: anonymous memory, a file mapping, or the vDSO

On `mmap(ANONYMOUS)`, a new VMA is inserted into the list. On `munmap`, VMAs are
split or removed as needed. `mprotect` updates the protection flags and rewalks the
page table.

## Demand Paging

Pages are not allocated at `mmap` time. The page fault handler
(`kernel/mm/page_fault.rs`) allocates and maps pages on first access:

1. Look up the faulting address in the VMA list.
2. If the VMA is anonymous: allocate a zeroed page and map it.
3. If the VMA is file-backed: read the appropriate page from the filesystem.
4. If no VMA covers the address: deliver `SIGSEGV`.

### Fault-Around

When handling a page fault, the kernel speculatively maps up to 64 surrounding pages
from the same VMA in a single pass. This amortizes the cost of sequential access
patterns (e.g., program load, `read` through a file mapping).

## Page Cache

A 64-entry LIFO page cache in the frame allocator (`platform/mm/`) keeps recently
freed frames warm. On allocation:

1. Try the per-CPU cache (lock-free fast path).
2. On cache miss, refill from the global ZONES allocator in a single lock hold.

For fault-around, `alloc_page_batch` drains the cache and zones in one call, avoiding
repeated lock acquisitions.

## Physical Frame Allocator

The buddy allocator (`buddy_system_allocator 0.11`) manages physical memory zones.
The platform exposes `alloc_pages(n)` and `free_pages(frame, n)` to the kernel.

Frame tracking uses a bitmap with raw byte scans and `(!byte).trailing_zeros()` for
fast free-bit location.

## Kernel Heap

The kernel heap uses `buddy_system_allocator` as the global allocator
(`#[global_allocator]`). Heap memory is carved from the kernel's physical memory
reservation at boot.

## vDSO

A 4 KB ELF shared object (`platform/x64/vdso.rs`) is mapped read+exec into every
process at address `0x1000_0000_0000`. It implements `__vdso_clock_gettime` entirely
in user space using RDTSC + fixed-point multiply (no syscall), achieving ~10 ns
latency vs ~87 ns with a syscall.

TSC calibration data (origin timestamp + `NS_MULT`) is written into the vDSO page
at boot and read via RIP-relative addressing at runtime.

The `AT_SYSINFO_EHDR` auxv entry points musl to the vDSO mapping.

## Address Space Operations

| Syscall | Implementation |
|---|---|
| `mmap` | Allocate VMA, optionally populate via demand paging |
| `munmap` | Remove VMAs, unmap pages, return frames to allocator |
| `mprotect` | Update VMA flags, remap pages with new permissions |
| `brk` | Extend/shrink the heap VMA |
| `madvise` | Stub (returns 0) |
| `mlockall` | Stub |
