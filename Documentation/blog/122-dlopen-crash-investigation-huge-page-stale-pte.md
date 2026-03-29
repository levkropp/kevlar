# Blog 122: Python dlopen crash — stale PTE investigation, musl tracing

**Date:** 2026-03-26
**Milestone:** M10 Alpine Linux

## Summary

Deep investigation of the Python C extension crash (`import math` SIGSEGV).
Built and deployed a patched musl dynamic linker with relocation tracing to
identify the exact failure point. Key findings:

1. **dlopen from C works perfectly** — all libraries (libcrypto, libssl, libz,
   even Python's math.so with libpython pre-loaded) load successfully from a
   dynamically-linked C test binary
2. **Crash is Python-process-specific** — only occurs when dlopen is called from
   within the Python interpreter process
3. **Reproduces under TCG** — not a KVM TLB coherency issue
4. **musl tracing reveals corrupt .gnu.hash data** — the dynamic linker's
   `find_sym` reads garbage from libpython's GNU hash table during symbol lookup

## Root cause analysis

### The crash mechanism

When Python calls `import math`, musl's dlopen loads `math.cpython-312.so` and
processes its RELA relocations. For each relocation with a symbol reference,
musl calls `find_sym` which searches the GNU hash tables of all loaded DSOs
(libpython, libc/ld-musl, python binary, math.so).

The crash occurs in `gnu_lookup_filtered()`:
```c
const size_t *bloomwords = (const void *)(hashtab+4);
size_t f = bloomwords[fofs & (hashtab[2]-1)];  // ← CRASH HERE
```

When `hashtab[2]` (bloom filter size) is 0, the expression `hashtab[2]-1`
underflows to `0xFFFFFFFF`, producing a massive array index that accesses
unmapped memory → SIGSEGV.

### What the musl trace revealed

Patched musl 1.2.6 with tracing in `reloc_all`, `do_relocs`, `find_sym2`,
`decode_dyn`, and `map_library`. Key output:

```
KTRACE reloc_all: math.cpython-312-x86_64-linux-musl.so
  base=0xa00a50000          ← correct (valloc region)
  DT_RELA=0x1340            ← correct (matches ELF parser)
  DT_RELASZ=0x9f0           ← correct (106 entries)
  rela_ptr=0xa00a51340      ← correct (base + DT_RELA)
  phase: JMPREL             ← OK
  phase: REL                ← OK
  phase: RELA               ← crashes during first entry's find_sym
    find_sym DSO: /usr/lib/libpython3.12.so.1.0
      ghashtab=0xa000b3348
      ght[0]=0x80f7f0       ← WRONG (should be ~1000, not 8.4 million)
      ght[2]=0x0            ← WRONG (should be ~256, not 0)
  SIGSEGV at 0xa07bbb248
```

### The corrupt data

The .gnu.hash section is at file offset 0x348 in libpython. The ON-DISK data
is correct:
```
file[0x348..0x368] = e903000075010000 0001000e00000000 ...
                     nbuckets=0x3e9  symoff=0x175  bloom=0x100  shift=0xe
```

But musl reads `0x80f7f0` at `ghashtab` (= `base + 0x348`). The value
`0x80f7f0` looks like a **relocated pointer** — it's `0xa00000000 + offset`
truncated. This suggests the page at `ghashtab` has been overwritten by RELA
relocation processing that patched a nearby address in the data segment.

### What we ruled out

- **KVM TLB coherency** — crash reproduces identically under TCG (software emulation)
- **Stale PTEs from huge pages** — added VMA boundary check to `prefault_cached_pages`,
  stale PTEs verified absent via `alloc_vaddr_range` clearing
- **mmap data corruption** — read() vs mmap() integrity test passes for all files
  including libcrypto.so.3 (4.3MB), libssl.so.3, and self-created 1MB files
- **Wrong mmap addresses** — `alloc_vaddr_range` returns correct addresses,
  `is_free_vaddr_range` properly detects VMA overlaps
- **ext4 filesystem corruption** — file content verified correct via pure-Python
  ELF parser reading from within the Kevlar process

## Fixes applied

1. **Huge page VMA boundary check** (`process.rs:prefault_cached_pages`):
   Don't create 2MB huge pages that extend beyond immutable file VMA boundaries
   into address space that will later be used by mmap

2. **`alloc_vaddr_range` stale PTE clearing** (`vm.rs`):
   Clear any existing PTEs in the returned address range before handing it to
   mmap. Prevents stale pages from `prefault_writable_segments` being reused
   for different files

3. **`alloc_vaddr_range` page-aligned advancement** (`vm.rs`):
   When skipping past a conflicting VMA, advance `valloc_next` to the
   page-aligned end (not the raw VMA end) to avoid sub-page overlaps

4. **MAP_FIXED huge page handling** (`mmap.rs`):
   Split 2MB huge pages before unmapping 4KB pages in the MAP_FIXED range

5. **`valloc_next` post-exec advancement** (`process.rs`):
   After all prefaulting during exec, advance `valloc_next` past all existing
   VMAs to prevent future mmap allocations from overlapping with prefaulted pages

6. **`prefault_writable_segments` VMA check** (`process.rs`):
   Only map pages that are within an actual VMA, preventing stale PTEs at
   page-aligned boundaries beyond segment ends

## New tests

- **Dynamically-linked dlopen test** (`testing/test_dlopen.c`):
  Tests dlopen of libcrypto, libssl, libz, stress with 100 VMAs,
  libpython + math.so — ALL PASS
- **mmap integrity tests** in `test_ext4_comprehensive.c`:
  1MB self-created file, /usr/bin/curl, /usr/lib/libcrypto.so.3,
  /usr/lib/libssl.so.3, Python extension .so files — ALL PASS
- **Long symlink tests** (>60 byte targets on ext4): 4 tests, ALL PASS
- **Pure-Python ELF parser**: dumps RELR/RELA sections and .gnu.hash data
  from within the Kevlar process (no C extensions needed)

## Remaining investigation

The .gnu.hash data is correct on disk and correctly demand-faulted, but becomes
corrupt by the time `find_sym` reads it. The leading hypothesis is that RELA
relocation writes to a nearby DATA segment page spill into the .gnu.hash page
if they share a physical page boundary.

**Next step:** Check whether the .gnu.hash section (read-only, in first PT_LOAD)
and the .dynamic/.got section (read-write, in data PT_LOAD) share a page-level
overlap at their segment boundaries in libpython.so.

## Test results

- **Contract tests:** 159/159 PASS
- **Ext4 comprehensive:** 37/39 PASS (2 expected static-dlopen failures)
- **dlopen from C:** ALL PASS (libcrypto, libssl, libz, stress, math+libpython)
- **Python pure:** 5/5 PASS (print, os, listcomp, sys, version)
- **Python C extensions:** FAIL (import math, import hashlib — SIGSEGV)
