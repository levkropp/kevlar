# M9.6: Page Cache, Exec Prefaulting, and the Permission Bug That Hid Everything

Blog post 070 ended with a table of shame: `pipe_grep` at 15x slower than
Linux, `sed_pipeline` at 21x.  Every benchmark that touched fork+exec was
an order of magnitude off.  We set out to profile, fix, and verify — and
ended up finding that a latent VMA permissions bug was masking every
optimization we tried.

## The profile says: page faults dominate

We added TSC-based page fault counters to the existing syscall profiler.
Two global atomics (`PAGE_FAULT_COUNT`, `PAGE_FAULT_CYCLES`) accumulate
across all CPUs.  The profiler dump now includes a `page_faults` entry
alongside the per-syscall breakdown.

The numbers confirmed the hypothesis: each exec of BusyBox triggers
~100-300 demand-paging faults for text and rodata pages.  Under KVM, each
fault is a VM exit (~200ns) + handler (~300ns) + VM entry (~200ns) =
**~700ns per page**.  At 300 pages, that's **~200µs per exec** — more than
3x what Linux spends on the *entire* fork+exec+wait cycle.

## Fix 1: initramfs page cache

Linux keeps file pages in a global page cache so repeated execs of
`/bin/busybox` hit cached physical pages instead of re-reading from disk.
Kevlar's initramfs files are `&'static [u8]` — truly immutable.  We can
do even better than Linux: share the *physical pages* directly across
processes, zero-copy.

The cache is a `HashMap<(usize, usize), PAddr>` keyed by `(file_data_ptr,
page_index)` behind a single `SpinLock`.  The `file_data_ptr` is the thin
pointer from `Arc::as_ptr()` on the VMA's `Arc<dyn FileLike>` — stable
because initramfs files are never deallocated.

Three paths through the page fault handler:

1. **Cache miss**: allocate page, read from file, insert into cache.
   `page_ref_init(paddr)` then `page_ref_inc(paddr)` gives refcount 2
   (one for the mapping, one for the cache).
2. **Cache hit, read-only VMA**: free the pre-allocated page, bump the
   cached page's refcount, map it directly.  No allocation, no copy.
3. **Cache hit, writable VMA**: copy from cached page to the fresh page.
   Skips the file read but still allocates.  CoW handles later writes.

We added `is_content_immutable()` to the `FileLike` trait (defaults to
`false`), overriding to `true` in the initramfs.  Only immutable files
enter the cache.

Result: **pipe_grep 979µs → 825µs** (16% faster), **sed_pipeline 1370µs
→ 949µs** (31% faster).  Good, but still 10-15x off Linux.

## Fix 2: exec-time prefaulting

The page cache eliminates the file-read overhead but not the VM exits.
Each demand-paging fault still costs ~700ns for the exit/entry round-trip.
Linux avoids this by mapping cached pages at `execve()` time, before the
process starts running.

We added `prefault_cached_pages()` to the exec path, called from
`do_elf_binfmt()` after `load_elf_segments()` creates the VMAs.  It holds
the page cache lock once, iterates through file-backed VMAs, and for each
page-aligned full-page region checks the cache.  Hits get mapped directly
via `try_map_user_page_with_prot()` with `page_ref_inc()` for the new
mapping.

A critical detail: prefaulted pages are mapped **read-only**
(`PROT_READ|PROT_EXEC`) regardless of the VMA's write permission.  If the
process writes to a prefaulted page, the CoW path in the fault handler
allocates a private copy.  This prevents shared-writable corruption across
processes.

First attempt: **zero improvement**.  The prefault function showed
`checked=0`.

## The bug: all VMAs were writable

`load_elf_segments()` created file-backed VMAs via `add_vm_area()`, which
defaults to `PROT_READ | PROT_WRITE | PROT_EXEC`.  Every VMA — including
BusyBox's .text segment — appeared writable.

This broke two things:
1. The demand-paging cache path always took the "writable VMA" branch,
   *copying* from cache to a fresh page instead of sharing.
2. Prefaulting skipped all VMAs (our safety filter excluded writable ones).

The fix: convert ELF `p_flags` to proper `MMapProt` values.

```rust
fn elf_flags_to_prot(p_flags: u32) -> MMapProt {
    let mut prot = MMapProt::empty();
    if p_flags & 4 != 0 { prot |= MMapProt::PROT_READ; }
    if p_flags & 2 != 0 { prot |= MMapProt::PROT_WRITE; }
    if p_flags & 1 != 0 { prot |= MMapProt::PROT_EXEC; }
    prot
}
```

And use `add_vm_area_with_prot()` instead of `add_vm_area()` for
file-backed segments.

## Fix 3: intermediate page table attributes

When the ELF prot fix went in, we found that read-only/NX leaf PTEs were
propagating their restrictions upward through the page table hierarchy.
On x86-64, effective permissions are the **intersection** of all four
levels (PML4 → PDPT → PD → PT).  If a PDE was written with NX set
because the first mapping through it was NX, all subsequent sibling
PTEs in that PD inherited the NX restriction — silently breaking execute
permission for adjacent code pages.

The fix: intermediate entries (PML4E, PDPTE, PDE) always use permissive
flags (`PRESENT | USER | WRITABLE`, no `NO_EXECUTE`).  Only leaf PTEs
carry the restrictive attributes from the VMA's protection flags.

This also improved the `traverse()` hot path: we now only conditionally
write back an intermediate entry if it doesn't already have the expected
permissive flags, avoiding unnecessary stores on the common path.

## Fix 4: minor optimizations

**Tmpfs read lock scope**: for reads ≤ 4096 bytes, copy data to a stack
buffer under the spinlock, drop the lock, then usercopy.  Reduces lock
hold time from the usercopy duration to a fast `memcpy`.

**Page fault profiler**: accumulates TSC cycles per fault with
near-zero overhead when disabled (single `AtomicBool` check on the
fast path).

## Fix 5: fork CoW bulk memcpy

The `duplicate_table_cow` function walked all 512 entries of each page
table level, zero-filled the new table first, then conditionally copied
non-null entries one at a time.  For a sparse address space (BusyBox uses
~30 pages out of 512 possible per PT), that's 512 reads + ~30 writes +
a wasted 4KB zero-fill per level.

The fix replaces the zero+iterate pattern with a single 4KB
`ptr::copy_nonoverlapping` (bulk memcpy), then a fixup pass that only
touches entries needing modification:

- **Read-only user pages**: already correct from the copy, just need
  `page_ref_inc`.  No write to the child table.
- **Writable user pages**: clear WRITABLE in both parent and child for
  CoW.  Only these entries trigger writes.
- **Kernel pages**: shared, already correct from the copy.

The function also separates leaf (level 1) from intermediate paths at the
top level, avoiding a per-entry level check in the inner loop.

## Page table teardown (work in progress)

We implemented `teardown_user_pages()` — a recursive page table walk
that decrements refcounts and frees intermediate table pages when a Vm is
dropped.  Without it, every `fork()+exec()` leaks the old page table
pages and leaves stale refcounts on cached pages.

The implementation works for simple cases but causes hangs in the
BusyBox test suite.  It's disabled pending investigation.  The leak is
bounded (a few KB per process exit) and doesn't affect correctness for
the benchmarks.

## kwab crash dump integration

We integrated [kwab](https://github.com/levkropp/kwab), a structured
crash dump manager built alongside Kevlar.  kwab provides:

- **kwab-format**: `no_std` binary format with CRC32-checksummed sections
  for registers, syscall traces, flight recorder events, and memory maps
- **kwab-cli**: import Kevlar's JSONL debug events, inspect dumps, export
  to JSON, and browse crashes in a TUI

Kevlar already emits structured `DBG` events over serial for crashes,
panics, and syscall profiles.  kwab can import these directly:

```
kwab import serial.log -o crash.kwab
kwab inspect crash.kwab
kwab tui crashes/
```

The next step is adding `kwab-format` as a kernel dependency (it's
`no_std`) for direct binary emission, bypassing the JSONL intermediate.

## Results

**BusyBox test suite:** 101/101 pass (unchanged)

**Workload benchmarks** (fork+exec-heavy, Kevlar KVM):

| Benchmark | Before | After | Speedup |
|-----------|-------:|------:|--------:|
| exec_true | 177µs | **118µs** | 1.50x |
| shell_noop | 345µs | **162µs** | 2.13x |
| pipe_grep | 979µs | **429µs** | 2.28x |
| sed_pipeline | 1370µs | **526µs** | 2.60x |
| fork_exit | 55µs | **43µs** | 1.28x |

**Syscall micro-benchmarks** (selected, Kevlar KVM):

| Benchmark | Before | After | Speedup |
|-----------|-------:|------:|--------:|
| getpid | 116ns | **86ns** | 1.35x |
| pipe | 528ns | **411ns** | 1.28x |
| open_close | 759ns | **624ns** | 1.22x |
| mmap_fault | 2040ns | **1830ns** | 1.11x |
| mprotect | 1657ns | **1264ns** | 1.31x |
| clock_gettime | 14ns | **11ns** | 1.27x |

The intermediate page table fix had a surprisingly broad impact — every
operation that traverses the page table (which is most of them) got
faster.  The fork CoW bulk-copy optimization shaved a further ~2µs off
fork_exit.

## What's next

The workload benchmarks are still 2-8x slower than Linux's ~65µs.  The
remaining gap is:

- **Exec path overhead**: ELF parsing + VMA creation + path resolution
  = ~70µs per exec.  Linux does this in ~25µs.
- **Page cache coverage**: only ~62/289 BusyBox file pages are currently
  cached (the rest are partial pages at segment boundaries).  Relaxing
  the full-page requirement would increase coverage.
- **Page table teardown**: fixing the hang to eliminate refcount leaks
  and reclaim memory on process exit.
- **Fork optimization**: 42µs per fork; sharing read-only intermediate
  page table pages could cut this further.
