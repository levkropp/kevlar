# Blog 123: Python dlopen FIXED — heap/mmap overlap, 59/59 Alpine tests pass

**Date:** 2026-03-26
**Milestone:** M10 Alpine Linux

## Summary

Four major advances:

1. **Python C extensions work** — `import math`, `import hashlib` now succeed
2. **Root cause found and fixed** — heap (brk) overlapped with mmap library region
3. **Cgroups v2 improvements** — cgroup.events file, test_cgroups_hang passes
4. **Native Alpine image builder** — `tools/build-alpine-full.py` (no Docker)

## Root cause: heap/mmap address space overlap

### The bug

When the kernel loaded a PIE binary (like Python) with a dynamic linker, it set
the heap bottom to `align_up(max(main_hi, interp_hi), PAGE_SIZE)` — right after
the loaded ELF segments. But `alloc_vaddr_range` (used by mmap for library
loading) ALSO allocated from the same region, starting at `valloc_next`.

Result: musl's `brk()` heap and musl's `mmap()` library mappings shared the
same virtual address range. When Python's malloc grew the heap via brk, it wrote
to addresses that were ALSO mapped as read-only library pages (libpython.so).

The kernel's MAP_PRIVATE CoW path created private page copies, but the malloc
writes corrupted the library's `.gnu.hash` table on the shared page. When
Python later called `dlopen("math.so")`, the dynamic linker's `find_sym`
function read garbage from the corrupted hash table → SIGSEGV.

### How we found it

1. **Patched musl 1.2.6** with tracing in `reloc_all`, `do_relocs`, `find_sym2`,
   `decode_dyn`, and `map_library` (built from source, deployed to Alpine rootfs)

2. **musl trace showed** correct `base`, `DT_RELA`, `ghashtab` at decode_dyn time,
   but corrupt `ghashtab[0..3]` when `find_sym` accessed it during dlopen

3. **Kernel CoW trace** showed writes to the .gnu.hash page from user IP in
   `__malloc_alloc_meta` — musl's malloc writing to the heap, which overlapped
   with the library address range

4. **`nm` on musl** confirmed the IP offset was in the malloc allocator, not the
   relocation code

### The fix

Reserve 256MB for the heap after loaded ELF segments, then advance `valloc_next`
past the reservation. This ensures `alloc_vaddr_range` never returns addresses
that overlap with the brk region:

```rust
// In do_elf_binfmt, dynamic linking path:
let new_heap_bottom = align_up(final_top, PAGE_SIZE);
vm.set_heap_bottom(new_heap_bottom);

// Advance valloc_next past 256MB heap reservation
let heap_reserve = new_heap_bottom + 256 * 1024 * 1024;
if heap_reserve > vm.valloc_next() {
    vm.set_valloc_next(heap_reserve);
}
```

### Result

```
sqrt2= 1.4142135623730951
TEST_PASS python3_math
TEST_PASS python3_hashlib
```

## Additional kernel fixes

### 1. prefault_cached_pages huge page boundary check

Don't create 2MB huge pages that extend beyond immutable file VMA boundaries.
Previously, a huge page for the interpreter could overlap with addresses later
used by mmap for ext4 library files.

### 2. alloc_vaddr_range improvements

- **Stale PTE clearing**: clear any existing PTEs in the returned range before
  handing it to mmap
- **Page-aligned advancement**: when skipping past a conflicting VMA, advance to
  `align_up(vma.end(), PAGE_SIZE)` instead of the raw VMA end

### 3. MAP_FIXED huge page handling

Split 2MB huge pages before unmapping 4KB pages in MAP_FIXED ranges.

### 4. prefault_writable_segments VMA check

Only map pages that are within an actual VMA, preventing stale PTEs at
page-aligned boundaries beyond segment ends.

### 5. mmap hint address validation

Reject mmap address hints below 0x10000 (64KB). musl passes the library's
`addr_min` (lowest p_vaddr, often ~0xa000) as a hint. Without this check, the
kernel would map libraries at tiny addresses where the dynamic linker computes
`base = map - addr_min ≈ 0`.

## Cgroups v2 improvements

### cgroup.procs PID 0 handling

Writing "0" to `cgroup.procs` now correctly maps to the current process (Linux
cgroup2 semantics). Previously returned ESRCH because PID 0 doesn't exist.

### cgroup.events file

Added `cgroup.events` control file with `populated` and `frozen` fields.

### Test results

`test_cgroups_hang` steps 1-7 all PASS, including the previously-hanging step 6e
(fork+exec busybox cat from child cgroup). The hang was caused by the
cgroup.procs write failing (ESRCH), so the test never actually ran from a child
cgroup.

### Remaining: OpenRC cgroups service hang

The OpenRC cgroups service still hangs when it moves to a child cgroup and execs
dynamic helpers. This is a separate issue from the Python dlopen crash — it
needs investigation of fork/exec behavior from non-root cgroups with dynamic
binaries.

## New test infrastructure

- **Patched musl 1.2.6** (`build/musl-debug/libc.so`): built from source with
  relocation tracing in dynlink.c
- **Dynamically-linked dlopen test** (`testing/test_dlopen.c`): tests dlopen of
  libcrypto, libssl, libz, stress with 100 VMAs, libpython + math.so,
  Python extension .so, and RELR/RELA analysis of libpython
- **Blog 122**: detailed investigation log with musl trace output

## Test results

- **Contract tests:** 159/159 PASS
- **Ext4 comprehensive:** 37/39 PASS
- **Cgroups test:** 7/8 PASS (step 8 = cleanup, expected)
- **Python pure:** 5/5 PASS
- **Python C extensions:** 2/2 PASS (math, hashlib)
- **dlopen from C:** ALL PASS (libcrypto, libssl, libz, stress, math+libpython)

## Native Alpine image builder

Added `tools/build-alpine-full.py` — builds a 512MB ext4 Alpine image without
Docker. Downloads Alpine minirootfs tarball, configures APK repos, networking,
OpenRC inittab, and creates the disk image with `mke2fs`.

The Makefile now auto-detects Docker availability and falls back to the native
builder when Docker isn't running. This prevents stale image state from
accumulating across test sessions — each `make build/alpine.img` creates a fresh
pristine image.

The stale image was the source of the OpenRC hang: previous test runs had
enabled the cgroups service and partially installed packages, leaving the ext4
filesystem in a corrupted state.

## Test results (final)

- **Ext4 comprehensive:** 36/38 PASS (2 = expected static-dlopen failures)
- **Alpine APK:** 59/59 PASS
  - OpenRC boot: PASS
  - curl HTTP + HTTPS: PASS
  - Python 3.12 install + 7 tests: ALL PASS
  - dlopen from C (6 tests): ALL PASS
  - Long symlinks (5 tests): ALL PASS
  - mmap integrity (4 tests): ALL PASS
- **Cgroups test:** 7/8 PASS (step 8 = cleanup, expected)

## What's next

1. Test `update-ca-certificates` (remove `-k` flag from curl HTTPS)
2. More Python C extension testing (socket, ctypes, json)
3. Cgroups PID 0 handling + OpenRC cgroups service enablement
4. Performance benchmarks to verify no regressions
