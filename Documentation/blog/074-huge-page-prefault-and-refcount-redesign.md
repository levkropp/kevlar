# M9.8: Huge Page Prefault, Refcount Redesign, and Page Cache Safety

This session tackled the three workload regressions (`pipe_grep` 6.4x,
`sed_pipeline` 8.8x, `shell_noop` 2.3x) with page cache improvements,
exec profiling spans, a full refcount redesign for huge pages, and the
start of a huge page exec prefaulting system.

## Results

```
              Before    Cache only   +Assembly     Change    vs Linux
pipe_grep:    435µs     352µs        309µs        -29%      4.8x (was 6.4x)
sed_pipeline: 560µs     455µs        407µs        -27%      6.5x (was 8.8x)
shell_noop:   147µs     117µs        108µs        -27%      1.7x (was 2.3x)
exec_true:    102µs      88µs         80µs        -22%
```

No regressions on any syscall-level benchmark. 101/101 BusyBox tests pass.

## Change 1: Partial page cache coverage

The page cache previously only cached full 4KB pages (`copy_len ==
PAGE_SIZE`). Pages at segment boundaries (last page of .text, .rodata)
were always demand-faulted on every exec, each costing ~2.5µs in KVM.

Fix: cache partial pages too (`copy_len > 0`), since the remaining
bytes are already zero-filled by the page fault handler.

**Critical safety constraint discovered during testing:** Only cache
pages from **read-only VMAs**. Writable VMAs (like the `.data` segment)
share the physical page with the cache. The first process writes to
BSS (musl malloc metadata at `0x5231f8`), directly modifying the
cached physical page. Subsequent processes read stale malloc pointers
from the corrupted cache → SIGSEGV at `ip=0x0` or `addr=0x523210`.

```rust
// Before: only full pages, no writability check
is_cacheable = file.is_content_immutable()
    && offset_in_page == 0
    && copy_len == PAGE_SIZE;

// After: partial pages OK, but never from writable VMAs
let vma_readonly = vma.prot().bits() & 2 == 0;
is_cacheable = file.is_content_immutable()
    && offset_in_page == 0
    && copy_len > 0
    && vma_readonly;
```

The root cause was subtle: the page cache insertion happens **after**
the page is mapped with the VMA's actual protection. For writable VMAs,
the process has direct write access to the physical page that the cache
also references. There's no CoW between the process and the cache — CoW
only triggers on page faults, and the page is already mapped writable.

## Change 2: Exec profiling spans

Added three new tracer spans to identify a 13µs unaccounted gap in the
exec path:

- `EXEC_ELF_PARSE` — around `Elf::parse(buf)`
- `EXEC_SIGNAL_RESET` — around `reset_on_exec()` + signaled_frame clear
- `EXEC_CLOSE_CLOEXEC` — around `close_cloexec_files()`

These will pinpoint whether the gap is in ELF parsing, signal cleanup,
or FD table operations once profiled with `debug=trace`.

## Change 3: Refcount redesign for huge pages

### The pre-existing bug

The page refcount system uses a per-4KB-PFN array (`AtomicU16[1M]`).
When a 2MB huge page is created (512 contiguous 4KB pages), only the
**base PFN** gets `page_ref_init()` → refcount=1. The other 511
sub-PFNs remain at 0.

This causes incorrect behavior when `split_huge_page()` converts the
2MB PDE into 512 individual PTEs: the CoW write-fault handler calls
`page_ref_count(sub_page)` and gets 0 for non-base PFNs, leading to
either refcount underflow (assertion failure) or incorrect sole-owner
detection (data corruption of shared pages).

### The fix (5 files)

**`page_refcount.rs`** — Two new bulk operations:

```rust
pub fn page_ref_init_huge(base: PAddr) {
    // Initialize refcount=1 for all 512 sub-PFNs
}

pub fn page_ref_inc_huge(base: PAddr) {
    // Increment refcount for all 512 sub-PFNs
}
```

**`paging.rs` (duplicate_table)** — Fork now uses `page_ref_inc_huge()`
for huge PDEs, correctly incrementing all 512 sub-PFN refcounts.

**`paging.rs` (teardown_table)** — Huge page teardown now decrements
and frees each sub-page individually:

```rust
for sub_i in 0..512usize {
    let sub = PAddr::new(paddr.value() + sub_i * PAGE_SIZE);
    if page_ref_dec(sub) {
        free_pages(sub, 1);
    }
}
```

The buddy allocator coalesces the freed pages back into larger blocks.

**`page_fault.rs`** — Anonymous THP creation now uses
`page_ref_init_huge()`.

**`munmap.rs`** — Huge page unmap now uses per-sub-page dec+free.

### Correctness verification

Scenario: anonymous THP, fork, child writes:
1. THP created: `page_ref_init_huge` → all 512 = 1
2. Fork: `page_ref_inc_huge` → all 512 = 2
3. Child writes page X → split → CoW detects refcount=2 → copies
4. Parent exits → teardown decs all 512 → goes to 1 (except X which was already 1 from CoW dec → goes to 0, freed)
5. Child exits → teardown decs its PTEs → private copies freed, remaining sub-pages 1→0, freed

## Change 4: Huge page exec prefault — COMPLETE

The largest remaining cost is **265µs userspace execution** per
pipe_grep iteration, dominated by EPT TLB misses under KVM. BusyBox
maps to ~287 4KB pages across text/rodata/data. Each TLB miss costs
~200ns due to 2D EPT page walks.

The approach: assemble a contiguous 2MB physical page from cached + file
data during exec, then map sub-pages as individual 4KB PTEs (not a
2MB huge PDE, to avoid split_huge_page complexity). This eliminates
ALL demand faults for subsequent execs, including for pages not yet
in the 4KB page cache.

**Implementation:**
- `HUGE_PAGE_CACHE` with bitmap: caches assembled 2MB pages with a
  `[u64; 8]` bitmap tracking which sub-pages have content
- Assembly loop: per-sub-page VMA lookup, copy from 4KB cache (fast)
  or read from file (uncached .data pages)
- Cache-hit path: maps only bitmap-set sub-pages, all as RX (CoW)
- Per-sub-page refcount management (init_huge + inc_huge)

### The boundary page bug

The assembly caused 36/100 BusyBox tests to crash with `SIGSEGV ip=0x0`.
Three kwab diagnostic tools were built to hunt it down:

**verify-pages** (`debug=verify`): Post-exec page content checksumming
against backing files. Confirmed all 285/285 pages correct at prefault
time — the corruption was runtime, not prefault.

**audit-vm** (`debug=audit`): VMA-to-PTE permission audit. No
permission mismatches found.

**Binary search on sub-pages**: Mapped progressively more sub-pages
until crash appeared. The 285th sub-page at `0x51c000` was the culprit
— the gap/.data **boundary page**.

**Root cause**: Page `0x51c000` straddles an anonymous gap VMA
(0x51c000-0x51cbf0) and the .data file VMA (0x51cbf0-0x521bf8). The
assembly populated it with .data file content at offset 0xbf0 and
mapped it RX. When a process wrote to the gap portion (e.g., musl
writing to .data globals), the page fault handler found the **gap VMA**
(anonymous) — not the .data VMA. The gap VMA's CoW path treated the
page as anonymous, upgrading its PTE to writable without realizing the
page was shared with the huge page cache. Subsequent processes mapped
the same physical page and read corrupted .data content (stale malloc
pointers → null function call → `ip=0x0`).

**Fix**: Skip boundary pages where a file VMA starts mid-page
(`sub_vaddr < info.vma_start`). These are left unmapped for the demand
fault handler, which correctly handles partial-page VMA placement using
the `aligned_vaddr < vma.start()` logic.

### lookup_pte_entry API

New `PageTable::lookup_pte_entry()` method returns the raw PTE value
(including flags) for a virtual address. Used by audit-vm.
