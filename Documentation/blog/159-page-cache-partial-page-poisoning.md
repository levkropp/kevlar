# Blog 159: Page Cache Partial Page Poisoning

**Date:** 2026-04-13

## The Bug

BusyBox processes (getty, sh) crash with `ip=0x0` — a NULL function
pointer call.  The crash rate is 40-60% on every boot, affecting both
single-CPU and SMP configurations.  The object at RBP is entirely
zeros: a function pointer table that was never populated.

```
SIGSEGV: null pointer access (pid=N, ip=0x0)
  RAX=0x0  RBP=<heap addr>  RSP=<stack addr>
  RIP=0x0  RFLAGS=0x10246  fault_addr=0x0
```

The RBP-relative function pointer table is ALL zeros.  This isn't a
vtable initialization failure or a COW race — the data was never
loaded into the page in the first place.

## Root Cause: Partial Pages in the Page Cache

BusyBox's ELF layout has two adjacent LOAD segments:

```
LOAD 0xeb000  -> 0x4eb000  filesz=0x312e0  (rodata, R)
LOAD 0x11cbd0 -> 0x51dbd0  filesz=0x3783   (data, RW)
```

The rodata segment ends at file offset `0x11C2E0` — only `0x2E0`
bytes into page index `0x11C`.  The rest of that 4KB page is zeros.
When the demand fault handler loads this page, it reads `0x2E0` bytes
of rodata content and zero-fills the remaining `0xD20` bytes.  The
page cache stores this partial page under key `(file, 0x11C)`.

The data segment starts at `p_vaddr=0x51dbd0`, which is NOT
page-aligned.  After page alignment, its VMA begins at `0x51d000`
with file offset `0x11C000` — the SAME page index `0x11C`.

When `prefault_cached_pages` assembles huge pages (2MB) covering both
segments, it finds page index `0x11C` already in the cache.  It reuses
the cached partial page for the data VMA's first page.  But the data
segment's function pointer table lives at offset `0xBD0` within the
page — well beyond the `0x2E0` bytes of real rodata content.  Those
function pointers are all zeros.

The result: BusyBox calls through a NULL function pointer and crashes.

## Why It Was Intermittent (40-60%)

The crash required a specific sequence:

1. The rodata boundary page at index `0x11C` must be demand-faulted
   and cached before the next exec of the same binary
2. The huge page assembly threshold (128 cached pages) must be met
3. The data segment's first page must be served from cache rather
   than freshly loaded from disk

Boot timing, process creation order, and page cache population all
varied enough that the poisoned page was reused roughly half the
time.  On boots where the data segment was demand-faulted
independently (without hitting the cache), the full `0x3783` bytes
were loaded correctly and everything worked.

## The Fix

Two changes to `kernel/mm/page_fault.rs`:

### 1. Only cache full pages

The page cache eligibility check changed from:

```rust
if copy_len > 0 {
    page_cache.insert((file, page_index), paddr);
}
```

to:

```rust
if copy_len == PAGE_SIZE {
    page_cache.insert((file, page_index), paddr);
}
```

Partial pages at segment boundaries can no longer poison the cache.
A page that contains only `0x2E0` bytes of real content will never be
served to a different VMA that needs different (fuller) content at the
same file offset.

### 2. Early permission fault handling

Added a fast path for PRESENT faults (permission violations like COW
or write-to-readonly) that skips file I/O entirely.  This preserves
modified MAP_PRIVATE page content and correctly handles huge page
splitting before copy-on-write.  Previously, a permission fault could
fall through into the file-loading path and accidentally overwrite a
modified private page with stale file content.

## Verification

8/8 boot tests pass with zero SIGSEGV crashes:
- 5 single-CPU boots: 0 crashes
- 3 SMP boots: 0 crashes

Before the fix: 40-60% crash rate across all configurations.

## Lesson

When caching pages from file-backed VMAs, partial pages at segment
boundaries share file page indices with adjacent segments.  The
adjacent segment may need different (fuller) content at the same file
offset.  The page cache must either track how many bytes of each page
are valid, or — the simpler approach — only cache pages that were
fully populated from the file.

This is a category of bug that's invisible in most ELF binaries.  It
only manifests when two LOAD segments share a file page boundary AND
the partial page gets cached before the overlapping segment is loaded.
BusyBox's compact static-PIE layout makes this common; dynamically
linked binaries with well-aligned segments rarely trigger it.

## Files Changed

- `kernel/mm/page_fault.rs` — partial page cache exclusion, early permission fault fast path
