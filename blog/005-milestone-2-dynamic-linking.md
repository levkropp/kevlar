# Milestone 2: Dynamic Linking Works on Kevlar

**Date:** 2026-03-08

---

Kevlar now runs dynamically linked musl binaries. A PIE (Position-Independent Executable) hello-world linked against `/lib/ld-musl-x86_64.so.1` boots, relocates, and prints its output:

```
/ # /bin/hello-dynamic
hello from dynamic linking!
/ # echo $?
0
```

This is the first time a dynamically linked binary has run on Kevlar. The kernel loads the main executable, detects `PT_INTERP`, loads the musl dynamic linker as a second ELF image, constructs the auxiliary vector, and jumps to the interpreter's entry point. The linker self-relocates, resolves the main executable's symbols, and transfers control.

## What it took

### The ELF loader rewrite

The original Kerla ELF loader only handled `ET_EXEC` (fixed-address executables). M2 required extending it for the full dynamic linking dance:

1. **PT_INTERP detection** — scan program headers for the interpreter path
2. **Dual ELF loading** — parse and map both the main executable and the interpreter
3. **PIE support** — `ET_DYN` main executables need a kernel-chosen base address, since their segment virtual addresses start near zero
4. **Auxiliary vector** — the dynamic linker needs `AT_PHDR` (relocated program headers), `AT_BASE` (interpreter load address), `AT_ENTRY` (main executable entry point), plus `AT_UID`/`AT_GID`/`AT_EUID`/`AT_EGID`/`AT_SECURE` for security checks

The loader allocates separate address ranges for the main executable and interpreter using `vm.alloc_vaddr_range()`, computes base offsets, and maps each `PT_LOAD` segment with the appropriate relocation.

### Three bugs that mattered

**Page fault handler: wrong offset for unaligned VMAs.** When a VMA starts mid-page (e.g., the interpreter's RW data segment at `0xafbe0`), the page fault handler needs to place file data at the VMA start's page offset. The existing code used the *faulting address's* page offset instead:

```rust
// Before (wrong): uses fault address offset
offset_in_page = unaligned_vaddr.value() % PAGE_SIZE;

// After (correct): uses VMA start offset
offset_in_page = vma.start().value() % PAGE_SIZE;
```

This one-line fix was the difference between the dynamic linker reading its `_DYNAMIC` section correctly and reading garbage from the wrong file offset. The corrupted `_DYNAMIC` table caused the linker to dereference null pointers during self-relocation.

**AT_PHDR for PIE executables.** For `ET_EXEC` binaries, `AT_PHDR` can point to the file header pages mapped at a fixed kernel-chosen location. For PIE (`ET_DYN`), the dynamic linker computes its load bias as `AT_PHDR - phdr[0].p_vaddr`. If `AT_PHDR` doesn't point to the *relocated* program headers within the main executable's mapped image, the load bias computation produces garbage and every relocation is wrong.

The fix: for PIE, set `AT_PHDR = main_base + e_phoff` instead of the file header page address. The program headers live within the first `PT_LOAD` segment and are faulted in on demand.

**Gap-fill VMAs for musl's allocator.** musl's `reclaim_gaps` scans the ELF image for unused bytes between `PT_LOAD` segments (page padding, alignment gaps) and feeds them to its malloc free list. These gaps are valid addresses within the image's page-aligned span, but Kevlar only created VMAs for the exact segment ranges. Any access to a gap address produced "no VMAs for address" and killed the process.

The fix: after mapping all `PT_LOAD` segments, walk the page-aligned ranges and fill gaps with anonymous VMAs — both between segments and within the page padding before/after each segment:

```rust
// Anonymous padding after segment end (within the last page)
if seg_end < page_end {
    let pad_len = page_end - seg_end;
    vm.add_vm_area(
        UserVAddr::new_nonnull(seg_end)?,
        pad_len,
        VmAreaType::Anonymous,
    )?;
}
```

This ensures every byte within the image's page-aligned extent is backed by a VMA. Static BusyBox doesn't hit this because its allocator uses `brk` exclusively; only the dynamic linker's internal allocator exercises `reclaim_gaps`.

### New syscalls

M2 added four new syscalls and fixed one existing one:

| Syscall | Status | Purpose |
|---------|--------|---------|
| `pread64` | Full | Read from file at offset without changing file position; used by the dynamic linker to read ELF segments |
| `madvise` | Stub | Returns 0 for all advice values; `MADV_DONTNEED` and `MADV_FREE` will need real implementations later |
| `futex` | Partial | `FUTEX_WAIT` and `FUTEX_WAKE` with per-address wait queues; enough for musl's threading primitives |
| `set_robust_list` | Stub | Accepts and ignores; real robust futex handling deferred to M3 |
| `set_tid_address` | Fixed | Was returning 0; now correctly returns the calling process's TID |

### MAP_FIXED semantics

The dynamic linker uses `mmap(MAP_FIXED)` to place segments at exact addresses. Linux's `MAP_FIXED` silently unmaps any existing mapping at the target range before creating the new one. Kevlar was returning `EINVAL` for overlapping ranges. The fix: `MAP_FIXED` now calls `vm.remove_vma_range()`, unmaps PTEs, and frees pages before creating the new mapping.

## The debugging story

The first test run crashed immediately with "null pointer access" — the dynamic linker's self-relocation code was reading corrupted data. Adding `warn!`-level traces to the ELF loader revealed the addresses were correct but the data was wrong.

The breakthrough came from examining the musl interpreter's program headers. Its RW data segment starts at virtual address `0xafbe0` — not page-aligned. When the kernel page-faulted on the page containing this segment (`0xaf000`), the handler placed file data at byte offset `0xdc0` (the fault address's page offset) instead of `0xbe0` (the VMA start's page offset). The `_DYNAMIC` table, 0x1e0 bytes into the segment, was being read from the wrong file offset, producing garbage DT_* tags.

After fixing the page fault handler, the second crash was "no VMAs for address 0xa00004ff8" — address `0x4ff8` within the main executable's page-aligned span, but not in any segment VMA. Disassembling the crash point revealed musl's `realloc` writing a chunk header into reclaimed gap memory. The gap-fill VMA solution resolved this permanently.

## What's next

**M3: GNU Coreutils + Bash.** The dynamic linking infrastructure opens the door to running more complex binaries. M3 targets Bash and GNU Coreutils, which need `clone` (threading), job control (`setpgid`, `tcsetpgrp`), symlinks, and `readlink`. The ELF loader and dynamic linker support built in M2 carry forward — every dynamically linked binary benefits.

The test binary (`/bin/hello-dynamic`) and its interpreter (`/lib/ld-musl-x86_64.so.1`) are included in the initramfs. Run `make run` and execute `/bin/hello-dynamic` at the shell prompt to see it work.
