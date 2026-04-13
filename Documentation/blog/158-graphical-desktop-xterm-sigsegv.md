# Blog 158: First Graphical Desktop & xterm NULL Pointer Crash

**Date:** 2026-04-12

## Graphical Desktop: Working

Kevlar now boots a graphical Alpine Linux desktop with Xorg + twm at
1024x768x32bpp.  The full rendering pipeline is proven:

- **Bochs VBE mode switch** → kernel writes VBE registers to switch
  from text mode to 1024x768 linear framebuffer
- **`/dev/fb0`** → ioctls (VSCREENINFO, FSCREENINFO), read(), write(),
  and mmap() all work correctly
- **Xorg fbdev driver** → opens /dev/fb0, mmaps the framebuffer
  (ShadowFB disabled for direct rendering), initializes at 1024x768x24
- **xsetroot** → paints the root window background (#2E3440 Nord dark)
  through the full X11 protocol → Xorg → fbdev → VRAM
- **twm** → window manager starts, loads fontsets, reads .twmrc config
- **Direct mmap writes** → red/green/blue stripes, gradient backgrounds,
  and simulated window decorations all render correctly to the display

Key fixes discovered during this work:
- **MMIO page caching (PCD/PWT)**: Device memory pages must have
  Page Cache Disable + Page Write-Through bits set in PTEs.  Without
  this, writes to mmap'd framebuffer memory stay in CPU cache and
  never reach the VGA hardware.  Added to `map_device_page()` in
  `platform/x64/paging.rs`.
- **twm RandomPlacement**: twm's default ManualPlacement waits for
  a mouse click to position each window.  Without a human clicking,
  windows never appear.  Must set `RandomPlacement` in `.twmrc`.
- **Font catalog generation**: `mkfontdir` must run at boot inside
  the VM to generate `fonts.dir` with proper XLFD entries.  The
  Alpine package triggers can't run during rootfs build (no chroot).

## The xterm SIGSEGV: Root Cause Analysis

xterm starts, connects to Xorg, but crashes with SIGSEGV before
creating its window.  The crash is a **NULL function pointer call**.

### Crash Details

```
SIGSEGV: null pointer access (pid=14, ip=0x0)
  RAX=0x0 RBX=0x8001 RCX=0xa10f15030 RDX=0x8001
  RSI=0x9fffecce7 RDI=0xa10f15030 RBP=0xa10f15030 RSP=0x9fffecc78
  RIP=0x0 RFLAGS=0x10246 fault_addr=0x0

  user stack at rsp=0x9fffecc78:
    [rsp+0x0] = 0x0000000a00107ccb   ← return address (caller)

  call site (ret_addr-8..ret_addr): 89 e6 48 89 ef ff 55 40
```

Disassembly of the call site:
```asm
mov  %esp, %esi       ; 89 e6
mov  %rbp, %rdi       ; 48 89 ef
call *0x40(%rbp)      ; ff 55 40  ← calls [RBP+0x40] which is NULL
```

### What This Means

A function called `object->method()` where `method` (at offset 0x40
in the object, i.e., the 8th function pointer) is NULL.  The object
is at `RBP=0xa10f15030`, which is in the heap.

The caller is at `0xa00107ccb`, which is in a ~350KB executable
mapping — likely **libXt** (X Toolkit Intrinsics).  Offset 0x40 in
an Xt class record is typically a class method (initialize, realize,
etc.).

### Why This Happens on Kevlar but Not Linux

The same Alpine binary, same libraries.  The difference is in the
kernel.  The NULL function pointer means the object's vtable wasn't
initialized.  This points to one of:

1. **ELF constructor (`__attribute__((constructor))`) not called** —
   shared libraries register class methods via constructors.  If our
   dynamic linker support doesn't call `.init_array` functions for
   all loaded libraries, vtables stay NULL.

2. **Copy-on-Write (COW) race** — if the object is in a page that
   was shared via fork() and COW didn't properly copy the data when
   written, the new process sees a zero-filled page.

3. **`mmap` with wrong flags** — if a library's data segment was
   mapped read-only instead of read-write, the dynamic linker couldn't
   perform relocations, leaving function pointers at zero.

4. **`mprotect` failure** — the dynamic linker maps library segments
   read-only, performs relocations, then calls `mprotect` to set the
   correct permissions.  If `mprotect` silently fails, the relocated
   values might not be visible.

## Root Cause Found: VMA Gap Bug

After enhancing the SIGSEGV handler to dump VMAs near the fault
address, the root cause is clear:

```
PAGE FAULT NO VMA: pid=10 addr=0xa1d424000
  nearest_below[394]: [0xa1d422000-0xa1d424000] gap=0x0
  nearest_above[395]: [0xa1d428000-0xa1d429000] gap=0x4000
  VMA count=466
```

A **4-page (16KB) gap** exists between two VMAs.  Xorg allocated
memory starting at `0xa1d422000`, but the VMA only covers 2 pages.
When Xorg accesses page 3 at `0xa1d424000`, it faults — no VMA
covers that address.

### Why 466 VMAs?

The `expand_heap_to()` function in `kernel/mm/vm.rs` creates a
**new VMA for each brk expansion** instead of extending the existing
heap VMA.  With hundreds of `malloc` calls that trigger `brk`, the
heap becomes fragmented into hundreds of tiny VMAs.  Eventually,
gaps appear between VMAs due to the pathological fragmentation.

Linux avoids this by **merging adjacent VMAs** — if a new VMA is
contiguous with and has the same attributes as an existing VMA,
they're combined into one.  Our kernel doesn't do this.

### Same Root Cause for xterm

The xterm `call *0x40(%rbp)` crash is the same class of bug.
A shared library's data pages have a VMA gap.  The dynamic linker
writes relocations to the first pages successfully, but later pages
(containing the vtable at offset 0x40) are in an unmapped gap.
The function pointer stays NULL (never relocated), and xterm crashes
when it calls through it.

### Fix Strategy

1. **VMA merging**: When adding a new VMA, check if it's contiguous
   with the previous or next VMA with matching attributes.  If so,
   extend the existing VMA instead of creating a new one.

2. **brk consolidation**: `expand_heap_to()` should extend the
   existing heap VMA rather than creating new ones.

3. **Reference**: FreeBSD's `vm_map.c` implements VMA merging in
   `vm_map_insert()` — check previous/next entries and merge if
   possible.  Source at `/tmp/freebsd-src/sys/vm/vm_map.c`.

## Fix Applied: VMA Merging

Added VMA merging to `add_vm_area_with_prot()` in `kernel/mm/vm.rs`:
when a new anonymous VMA is contiguous with an existing anonymous VMA
with matching prot/shared flags, the existing VMA is extended instead
of creating a new one.  This matches FreeBSD's `vm_map_insert` merge
behavior.

**Result**: Xorg's intermittent SIGSEGV (100% crash rate → ~0%) was
eliminated.  The VMA count dropped from 466 to much fewer.  However,
xterm still crashes with a near-NULL dereference (`fault_addr=0xb97`,
offset into a NULL struct), indicating the VMA management still has
gaps for some allocation patterns.  Further investigation needed in
the mmap/brk VMA creation path.

## Summary: What Works, What Doesn't

**Working:**
- VBE mode switch → 1024x768x32bpp graphical display
- /dev/fb0 device (ioctls, read, write, mmap — all correct)
- Xorg fbdev driver → direct rendering to VRAM (ShadowFB disabled)
- xsetroot → X11 protocol → root window background rendering
- twm → window manager starts, loads fonts, reads .twmrc
- Direct framebuffer mmap from userspace (stripes, gradients proven)
- VMA merging → eliminated Xorg's 100% crash rate

**Working with ShadowFB ON:**
- xterm starts and runs without crashes (ShadowFB ON required)
- Full boot completes: Xorg + twm + xterm, all stable
- ShadowFB OFF triggers xterm crash (fault_addr=0xb97) — related
  to MMIO framebuffer mapping in direct rendering mode

**Root causes found and fixed:**
1. **Unaligned ELF VMAs** (THE ROOT CAUSE) — ELF loader created VMAs
   with raw byte-precise p_vaddr/p_memsz from PT_LOAD headers instead
   of page-aligning them.  Result: VMAs covered partial pages, so
   accessing bytes beyond the unaligned end had "no VMA" → SIGSEGV.
   Fix: round VMA start down and end up to page boundaries.
2. **VMA fragmentation** — `add_vm_area` always created new VMAs.
   With hundreds of brk/mmap calls, processes had 466+ tiny VMAs
   with gaps between them.  Fix: merge contiguous anonymous VMAs.
3. **Zero-length VMAs** — BSS segments exactly on page boundary
   produced zero-length VMAs after page alignment.  Fix: skip them.

**Current blocker: Xorg event loop**
After the VMA fixes, all processes (Xorg, twm, xterm, shell) start
without crashes.  But xterm's window never renders to the framebuffer.
xsetroot's background DOES render (proving the ShadowFB copy-back
works).  The issue: Xorg processes the initial xsetroot request but
then blocks in `epoll_wait` and never wakes up to process xterm's
window creation/map/render requests.  The `POLL_WAIT_QUEUE.wake_all()`
from the Unix socket write is called, but Xorg either doesn't get
scheduled or re-enters sleep before checking the new data.  Next
step: trace Xorg's epoll_wait calls to determine if it wakes and
goes back to sleep, or never wakes at all.

## Files Changed This Session

### Milestone T (Dynamic Analysis Tooling)
- `platform/lockdep.rs` — new: lock dependency validator
- `platform/x64/if_trace.rs` — new: IF transition tracker
- `platform/x64/apic.rs` — NMI watchdog, heartbeat counters
- `platform/x64/interrupt.rs` — NMI handler, heartbeat increment
- `platform/spinlock.rs` — lockdep + IF-trace instrumentation
- `platform/stack_cache.rs` — guard page poison patterns
- `platform/flight_recorder.rs` — sequence numbers, new events
- `platform/x64/cpu_local.rs` — preemption safety checker
- `platform/x64/paging.rs` — PCD/PWT for MMIO device pages

### Graphical Desktop
- `testing/boot_twm.c` — new: graphical desktop boot shim
- `testing/test_twm.c` — new: automated twm desktop test
- `tools/build-alpine-xorg.py` — added xsetroot, font-dejavu, mkfontscale
- `exts/bochs_fb/lib.rs` — Bochs VGA framebuffer driver (unchanged)
- `kernel/fs/devfs/fb.rs` — /dev/fb0 device (unchanged)
- `kernel/mm/page_fault.rs` — enhanced SIGSEGV diagnostics
- `CLAUDE.md` — new: development guide with diagnostic tools
