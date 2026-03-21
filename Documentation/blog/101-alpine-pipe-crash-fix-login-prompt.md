# Blog 101: Alpine pipe crash fix — PIE relocation pre-faulting + login prompt

**Date:** 2026-03-21
**Milestone:** M10 Alpine Linux

## Context

Blog 100 got Alpine Linux 3.21 booting on Kevlar with BusyBox init, all
sysinit commands completing, and a getty on ttyS0. But shell pipes crashed:
`sh -c "echo hello | cat"` → SIGSEGV at address 0x3d. This blocked piped
commands, command substitution, and `apk` package management.

---

## Investigation

### Narrowing down

Built 7 test programs to isolate the crash:

| Test | Result | Method |
|------|--------|--------|
| Static busybox fork+pipe | PASS | fork+exec, static binary |
| Dynamic busybox fork+exec | PASS | fork+exec of Alpine busybox |
| Dynamic busybox vfork+pipe | PASS | vfork+exec with pipe |
| Alpine shell simple command | PASS | `sh -c "echo nopipe"` |
| Alpine shell pipe | CRASH | `sh -c "echo hello \| cat"` |
| Alpine shell cmd substitution | CRASH | `sh -c "echo $(echo foo)"` |

Key finding: only BusyBox shell's **internal fork** crashed (where the child
runs a builtin without exec). All fork+exec paths worked fine.

### Tracing the crash

**Syscall trace** (debug=syscall) revealed:
- The fork children (PIDs 4, 5) had only 4 syscalls: `set_tid_address`,
  `rt_sigprocmask` ×2, `close(0)`, then SIGSEGV
- **No execve** — these were fork children running builtins, not exec'd processes

**Register dump** at crash point:
```
RDI=0x40  RBP=0xa0016c1a8  RSP=0x9ffffe8f8  RBX=0xa00000000
```

**Disassembly** showed the crash at musl's `aligned_alloc` → `movzbl -3(%rdi)`.
The allocator tried to read a chunk header at address `0x40 - 3 = 0x3D`.

**Stack trace** revealed the caller: BusyBox's shell cleanup function at
`0x41513` calling `free(ptr)` where `ptr = [RBX + 0x20]`.

### Finding the corrupt value

BusyBox loads a linked list head from a global variable via RIP-relative
addressing: `mov 0x84b1d(%rip),%rbx → loads from 0xa000c6010`.

**Page trace tool** (`platform/page_trace.rs`) verified:
- The page at `0xa000c6000` has **correct data** in both parent and child
  after fork (same physical page via CoW, value = `0xa00172440`)
- The node at `0xa00172440` has a field at offset 0x20 containing **0x40**

### Root cause: unpatched PIE relocations

`0x40` is the raw ELF `e_phoff` (program header offset) value from the
busybox binary file. In a PIE binary, the dynamic linker patches data
pointers by adding the load base (`0xa00000000`). The correct runtime
value should be `0xa00000040`.

**The patch was never applied** because the page containing this data
was **never demand-faulted by the parent process**. The dynamic linker
only patches pages it accesses during initialization. Pages that aren't
demand-faulted retain their raw file data.

After `fork()`, when the child accesses a page that the parent never
faulted, the page fault handler reads the raw file data (unpatched
pointers), not the parent's CoW data (which doesn't exist for unfaulted
pages).

This only affects **writable data segments** of PIE binaries, because:
1. Read-only segments (`.text`, `.rodata`) don't need relocation patching
   at the page level (RIP-relative addressing handles it)
2. Writable segments (`.data`, `.got.plt`) contain absolute pointers that
   the dynamic linker patches by writing to the pages
3. If a writable page is never written to by the dynamic linker (because
   the relocation targets on that page aren't accessed during init), the
   page stays as raw file data

---

## Fix

Eagerly pre-fault all writable `PT_LOAD` segment pages during `execve`,
reading file data into physical pages and mapping them before returning to
userspace. This ensures:

1. All data pages are populated with file content
2. The dynamic linker can patch ALL relocations (not just demand-faulted ones)
3. After fork, the child's CoW page table references correctly-patched pages

```rust
// In setup_userspace, after load_elf_segments:
for phdr in elf.program_headers() {
    if phdr.p_type == PT_LOAD && phdr.p_flags & 2 != 0 && phdr.p_filesz > 0 {
        // Pre-fault each page in the writable data segment
        for page_addr in (first_page..end_page).step_by(PAGE_SIZE) {
            let paddr = alloc_page(USER)?;
            executable.read(file_offset, &mut page_buf[..copy_len], ...)?;
            vm.page_table_mut().map_user_page_with_prot(vaddr, paddr, prot);
        }
    }
}
```

This matches Linux's behavior: writable data segments are populated
eagerly during exec, not lazily demand-faulted.

~30 lines of code. Zero performance impact on existing benchmarks.

---

## Debug tooling built

- **`platform/page_trace.rs`**: `dump_pte()` walks all 4 x86_64 paging
  levels and reads physical page content; `dump_stack()` reads the user
  stack via page table translation; `read_user_qword()` reads arbitrary
  user memory from any process's page table
- **SIGSEGV register dump**: RAX-R15 + stack contents at crash point
- **PML4/PDPT entry enumeration** in fork path
- **7 isolation test programs** for targeted reproduction

---

## Results

| Metric | Before | After |
|--------|--------|-------|
| `sh -c "echo hello \| cat"` | SIGSEGV | **hello** |
| `sh -c "echo $(echo foo)"` | SIGSEGV | **foo** |
| Alpine getty login prompt | Not reached | **kevlar login:** |
| Contract tests | 118/118 | 118/118 |
| Benchmarks | 0 regression | 0 regression |
| ext4 integration | 30/30 | 30/30 |

---

## Alpine boot status

```
=========================================
  Alpine Linux 3.21 running on Kevlar!
=========================================
Linux kevlar 6.19.8 Kevlar x86_64 Linux
--- pipe test ---
hello
=========================================
  All tests passed!
=========================================

Welcome to Alpine Linux 3.21
Kernel 6.19.8 on an x86_64 (/dev/ttyS0)

kevlar login:
```

BusyBox init, shell pipes, command substitution, and getty all work.
Next: fix getty respawn fd inheritance, implement pivot_root for OpenRC.
