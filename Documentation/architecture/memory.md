# Memory Management

## Virtual Address Space Layout (x86\_64)

```
0x0000_0000_0000 – 0x0000_0009_ffff_ffff   User space (~40 GB)
0x0000_000a_0000_0000                       VALLOC_BASE / USER_STACK_TOP
0x0000_000a_0000_0000 – 0x0000_0fff_0000_0000   VALLOC region (~245 TB)
0x1000_0000_0000                            vDSO (single 4 KB page, PML4 index 32)
0xffff_8000_0000_0000+                      Kernel (higher half, direct-mapped physical)
```

The user stack grows downward from `USER_STACK_TOP` (default 128 KB). The VALLOC region
is used for `mmap` allocations. The vDSO sits above VALLOC in its own PML4 entry.

## VMAs

User virtual memory is tracked as a list of `VmArea` structs in `kernel/mm/vm.rs`.

```rust
pub struct VmArea {
    start: UserVAddr,
    len: usize,
    area_type: VmAreaType,
    prot: MMapProt,  // PROT_READ | PROT_WRITE | PROT_EXEC
}

pub enum VmAreaType {
    Anonymous,
    File {
        file: Arc<dyn FileLike>,
        offset: usize,
        file_size: usize,  // For BSS: file_size < VMA len
    },
}
```

The `Vm` struct owns the VMA list and page table:

```rust
pub struct Vm {
    page_table: PageTable,
    vm_areas: Vec<VmArea>,
    valloc_next: UserVAddr,
    last_fault_vma_idx: Option<usize>,  // Temporal locality cache
}
```

VMA lookup uses a linear scan with temporal locality optimization — the last-hit VMA
index is cached and checked first, which is effective because consecutive page faults
tend to hit the same VMA.

### mmap

On `mmap(MAP_ANONYMOUS)`, a new VMA is inserted. Large anonymous mappings (>= 2 MB)
are 2 MB-aligned to enable transparent huge pages. `MAP_FIXED` unmaps any existing
pages in the range first, decrementing refcounts and freeing sole-owner pages.

No physical pages are allocated at `mmap` time — all pages are demand-faulted on
first access.

### munmap

`munmap` splits VMAs at the unmap boundaries, walks the affected page table entries,
decrements refcounts, and frees pages whose refcount drops to zero.

### mprotect

`mprotect` updates VMA flags, splits VMAs at boundaries if needed, and rewalks the
page table to update PTE permission bits. TLB invalidation uses batch local `invlpg`
plus a single remote IPI (O(1) IPIs regardless of page count).

### brk

`brk` expands or shrinks the heap VMA. Like `mmap`, no physical pages are allocated —
they are demand-faulted. Shrinking unmaps pages and frees frames.

## Demand Paging

Pages are not allocated at `mmap` time. The page fault handler
(`kernel/mm/page_fault.rs`) allocates and maps pages on first access:

1. Allocate a fresh page **before** acquiring the VM lock (minimizes lock hold time).
2. Look up the faulting address in the VMA list via `find_vma_cached`.
3. Determine the content:
   - **Anonymous**: zero-filled page.
   - **File-backed**: check the page cache. On hit, share the physical page (read-only)
     or copy it (writable mapping). On miss, read from the file and cache the result.
4. If no VMA covers the address: deliver `SIGSEGV` with crash diagnostics.

### Transparent Huge Pages

If the faulting address falls within a 2 MB-aligned anonymous region and the
corresponding PDE is empty, the fault handler allocates a single 2 MB huge page
instead of 512 individual 4 KB pages:

```rust
// Huge page fast path: 2MB-aligned, anonymous, PDE empty
if is_anonymous && is_2mb_aligned(vaddr) && pde_is_empty(vaddr) {
    let huge_paddr = alloc_huge_page()?;  // Order-9, 512 pages
    zero_huge_page(huge_paddr);
    map_huge_user_page(vaddr, huge_paddr, prot);
    return Ok(());
}
```

When a later operation needs 4 KB granularity on part of a huge page (e.g., `mprotect`
on a sub-range, or a CoW write fault), the huge page is split into 512 individual PTEs
preserving the original flags.

### Fault-Around

When handling a 4 KB page fault, the kernel speculatively maps up to 16 surrounding
pages from the same VMA in a single pass. This amortizes the cost of sequential access
patterns (program load, file reads). Fault-around respects VMA boundaries and does not
cross 2 MB huge page boundaries.

## Copy-on-Write

Fork uses copy-on-write (CoW) to avoid copying the entire address space:

```rust
// During fork: duplicate page tables with CoW
fn duplicate_table_cow(parent_pml4: PAddr) -> PAddr {
    // Walk PML4 → PDPT → PD → PT recursively
    // For each user-writable leaf PTE:
    //   1. Increment page refcount
    //   2. Clear WRITABLE bit in BOTH parent and child PTEs
    // Read-only pages (code, rodata): shared without refcount bump
}
```

On a write fault to a CoW page:

```rust
// Write fault on a present, non-writable page in a writable VMA
let old_paddr = lookup_paddr(vaddr);
let refcount = page_ref_count(old_paddr);

if refcount > 1 {
    // Shared page: allocate new, copy content, decrement old refcount
    let new_paddr = alloc_page()?;
    copy_page(new_paddr, old_paddr);
    page_ref_dec(old_paddr);  // May free if drops to 0
    map_writable(vaddr, new_paddr);
} else {
    // Sole owner: just make it writable (no copy needed)
    update_pte_flags(vaddr, WRITABLE);
}
```

2 MB huge pages also participate in CoW: a write fault on a shared huge page allocates
a new 2 MB page and copies the full 2 MB.

## Page Refcount Tracking

Per-page `u16` refcounts are stored in a flat array indexed by `paddr / PAGE_SIZE`.
Maximum tracked physical memory: 4 GB (1M pages). Refcounts are manipulated under the
page table lock with `page_ref_inc` / `page_ref_dec` / `page_ref_count`.

## Physical Frame Allocator

The buddy allocator (`buddy_system_allocator`) manages physical memory in up to 8 zones.
A 64-entry LIFO page cache sits in front for fast single-page allocation:

```
alloc_page()
  ├─ Try page cache (lock_no_irq, ~5 ns uncontended)
  └─ On miss: refill cache from buddy zones in single lock hold

alloc_page_batch(n)   # Used by fault-around
  ├─ Drain page cache
  └─ Allocate remaining from buddy directly

alloc_huge_page()     # 2 MB = order-9
  └─ Buddy allocator (returns dirty memory, caller zeroes)
```

### EPT Pre-Warming

At boot under KVM, the allocator pre-warms Extended Page Table entries by allocating
and freeing 2 MB blocks. This eliminates first-touch EPT violation latency (~13 µs
down to ~200 ns per page fault).

## Page Cache

File-backed pages are cached by the VFS layer. On a file-backed page fault:

- **Immutable file** (e.g., initramfs binaries): share the physical page directly via
  refcount — no copy needed for read-only mappings.
- **Writable mapping**: copy the cached page into a fresh frame (CoW-style).
- **Cache miss**: read from the filesystem into a fresh page, then cache it.

## Kernel Heap

The kernel heap uses `buddy_system_allocator::LockedHeapWithRescue` as the
`#[global_allocator]`. When the heap needs more memory, it requests 4 MB chunks from
the physical page allocator.

## vDSO

A hand-crafted 4 KB ELF shared object (`platform/x64/vdso.rs`) is mapped read+exec
into every process at `0x1000_0000_0000`. It implements `__vdso_clock_gettime` entirely
in user space:

```
rdtsc
sub rax, [tsc_origin]       ; delta = current TSC - boot TSC
mul [ns_mult]               ; 128-bit multiply
shrd rax, rdx, 32           ; nanoseconds = (delta * mult) >> 32
div 1_000_000_000           ; seconds and remainder
mov [rsi], rax              ; tp->tv_sec
mov [rsi+8], rdx            ; tp->tv_nsec
```

TSC calibration data (`tsc_origin` and `ns_mult`) is baked into the vDSO page at boot.
The `AT_SYSINFO_EHDR` auxv entry tells musl/glibc where the vDSO is mapped.

Performance: ~10 ns per `clock_gettime(CLOCK_MONOTONIC)`, 2x faster than Linux KVM.

## PCID (Process Context Identifiers)

On CPUs that support PCID (detected at boot), each address space receives a 12-bit TLB
tag. Context switches load the new PCID into CR3 without flushing the entire TLB,
preserving entries from other processes.

## Address Space Operations

| Syscall | Implementation |
|---|---|
| `mmap` | Allocate VMA, demand-page on first access, 2 MB-align large anonymous mappings |
| `munmap` | Split/remove VMAs, unmap pages, decrement refcounts |
| `mprotect` | Update VMA flags, remap PTEs, batch TLB invalidation |
| `brk` | Extend/shrink heap VMA |
| `madvise` | Stub (returns 0) |
| `mlockall` | Stub |
