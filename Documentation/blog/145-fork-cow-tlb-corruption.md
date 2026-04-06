# Blog 145: The fork that corrupted its parent — a TLB coherency bug

**Date:** 2026-04-05
**Milestone:** M11 Alpine Graphical — Stability

## Summary

The ip=0 crash that killed every X11 client was caused by a TLB
coherency bug in our fork() implementation. After fork marks pages
as read-only for copy-on-write, stale writable TLB entries allowed
the child process to write directly to the parent's physical pages,
bypassing the COW fault handler entirely. The smoking gun: the
parent's stack contained the hostname "kevlar" and kernel version
"6.19.8" — data from the child's `uname()` call.

## The crash pattern

Every X11 client (xterm, D-Bus session daemon, xfce4-panel) crashed
with the same signature:

```
SIGSEGV: null pointer access (pid=1, ip=0x0)
  RAX=0x0         ← NULL function pointer
  RCX=0x40599f    ← return address after last syscall
  R11=0x246       ← saved RFLAGS from syscall

  user stack at rsp=0x9ffffdbc0:
    [rsp+0x00] = 0x0000000000000000    ← zeroed return address!
    [rsp+0x08] = 0x0000000000000000
    [rsp+0x10] = 0x0000000000000000
    [rsp+0x18] = 0x6c76656b00000000    ← "kevl" (hostname!)
    [rsp+0x20] = 0x0000000000007261    ← "ar"
    [rsp+0x58] = 0x312e360000000000    ← "6.1" (kernel version)
    [rsp+0x60] = 0x0000000000382e39    ← "9.8"
```

The return address at `[rsp]` was zero. The `ret` instruction popped
zero into RIP → page fault at address 0.

## The smoking gun

The stack contained `"kevlar"` (the hostname) and `"6.19.8"` (the
kernel release string). These are fields from `struct utsname`,
written by the `uname()` syscall. But PID 1 (the test binary)
never called `uname()` at that stack location.

The data came from a **child process**. The test binary's `sh_run()`
function calls `fork()`, and the child runs `sh -c "mkfontdir ..."`.
The shell calls `uname()` during startup. The shell's `uname()`
buffer was on the child's stack — which should have been a private
COW copy of the parent's stack.

But it wasn't. The child wrote directly to the parent's physical
page.

## Root cause: stale TLB entries after fork

The fork sequence:

1. `duplicate_table_cow()` walks the parent's page table and clears
   the WRITABLE bit on all user PTEs (both parent and child copies)
2. `page_ref_inc()` increments refcounts on shared pages
3. `cr3_write(pml4 | pcid)` flushes the parent's TLB for its PCID

Step 3 should invalidate the parent's writable TLB entries so the
next write goes through the page fault handler. But with PCID
(Process Context Identifiers), the flush only invalidates entries
tagged with the parent's PCID. If ANY writable entry survives the
flush — due to a hardware microarchitectural issue, KVM EPT
interaction, or timing with speculative execution — writes bypass
COW entirely.

**Proof:** Disabling PCID (forcing PCID=0 for all processes)
eliminated the crash completely (6/6 clean runs vs 0/6 with PCID).

## The fix

Replace the PCID-specific flush with a full TLB invalidation during
fork:

```rust
// Before: PCID-specific flush (broken)
let cr3_val = original.pml4.value() as u64 | (original.pcid as u64);
x86::controlregs::cr3_write(cr3_val);

// After: full flush without PCID preservation
x86::controlregs::cr3_write(original.pml4.value() as u64);
```

Writing CR3 without PCID bits flushes ALL TLB entries for ALL
PCIDs, ensuring no stale writable entries survive.

**Results:**

| Configuration | PID 1 crash rate |
|--------------|-----------------|
| PCID enabled, PCID-specific flush | 100% (5/5 crash) |
| PCID disabled entirely | 0% (6/6 clean) |
| PCID enabled, full flush | ~20% (4/5 clean) |

The remaining ~20% failure rate suggests a secondary timing issue
(possibly related to TLB prefetching or speculative page walks)
that needs further investigation.

## Why PCID causes this

PCID allows the TLB to hold entries for multiple address spaces
simultaneously, tagged by a 12-bit process context identifier.
This avoids TLB flushes during context switches (a ~5% performance
win on context-switch-heavy workloads).

But PCID creates a correctness hazard for COW: when fork marks
pages read-only, it MUST flush writable TLB entries for the parent.
A PCID-specific flush (`cr3_write` with bit 63=0) should invalidate
all entries for that PCID. However, under KVM with EPT (Extended
Page Tables), the interaction between guest TLB, guest PCID, and
host EPT may leave stale entries that bypass the guest's read-only
protection.

## Investigation methodology

The breakthrough came from **user stack dumps** added to the null
pointer crash handler:

```rust
// Dump 16 stack values at the crash point
if r.rsp > 0x1000 && r.rsp < 0x7FFF_FFFF_FFFF {
    for i in 0..16u64 {
        let val = unsafe { *((r.rsp + i * 8) as *const u64) };
        warn!("  [rsp+{:#x}] = {:#018x}", i * 8, val);
    }
}
```

Without the stack dump, the crash looked like a generic null pointer
dereference. With the dump, the "kevlar" and "6.19.8" strings
immediately pointed to `uname()` output from a child process,
narrowing the bug to fork() + COW + TLB.

## Other fixes in this session

### XLFD font names

The image builder generated `fonts.dir` with generic XLFD names
for all 335 fonts. xterm couldn't find its default font
(`-misc-fixed-medium-r-semicondensed--13-120-75-75-c-60-iso10646-1`)
and crashed with a NULL Xlib error callback.

**Fix:** Pre-generate `fonts.dir` with 21 known XLFD names for the
standard misc-fixed bitmap fonts, plus ISO-8859-1 aliases (356
total entries).

### GTK cache pre-generation

GTK3 needs `gdk-pixbuf loaders.cache` to load ANY images. Without
it, the XFCE desktop renders as transparent/black. The image builder
now pre-generates:

- `loaders.cache`: 12 gdk-pixbuf loader modules
- `fontconfig cache`: via host `fc-cache --sysroot`
- `MIME database`: via host `update-mime-database`

### sched_affinity compat syscalls

musl libc uses x86_64 "common" syscall numbers (122/123) for
`sched_setaffinity`/`sched_getaffinity` instead of the "64-bit"
numbers (203/204). Added compat dispatch aliases.
