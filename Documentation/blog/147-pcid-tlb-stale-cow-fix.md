# Blog 147: The PCID TLB stale entry bug — phantom refcount root cause

**Date:** 2026-04-06
**Milestone:** M11 Alpine Graphical — Stability

## Summary

The "phantom refcount" bug (Blog 146, Bug 2) — where a physical page
had refcount=1 but two processes mapped it — was caused by **stale
WRITABLE TLB entries on remote CPUs after fork()**. The fix: allocate
a fresh PCID for the parent after fork, so stale entries tagged with
the old PCID are never used again.

## The bug

After `fork()`, `duplicate_table_cow` clears the WRITABLE bit on the
parent's PTEs (making them copy-on-write). The code then flushes the
**current CPU's TLB** via a CR3 write:

```rust
let flush_cr3 = original.pml4.value() as u64 | (original.pcid as u64);
x86::controlregs::cr3_write(flush_cr3);
```

This only flushes the local CPU. With PCID enabled, context switches
use `cr3_write(pml4 | pcid | (1<<63))` — the no-invalidate bit (63)
means the target CPU's TLB is **not flushed**. Old entries for the
same PCID persist across switches.

### The race

```
1. Parent (PCID=5) runs on CPU 0, writes to address X
   → CPU 0 TLB: PCID 5, X → page P (WRITABLE)

2. Parent migrates to CPU 1 (normal scheduling)
   → CPU 1 TLB: PCID 5, X → page P (WRITABLE) (via page walk)

3. Parent forks on CPU 1
   → PTE for X changed to read-only, P refcount incremented to 2
   → CPU 1 TLB flushed (CR3 write) ✓
   → CPU 0 NOT flushed — stale WRITABLE entry persists ✗

4. Parent migrates back to CPU 0
   → switch() does cr3_write(pml4 | PCID=5 | bit63=1)
   → No-invalidate! CPU 0 still has stale entry from step 1

5. Parent writes to X on CPU 0
   → Stale TLB entry allows write WITHOUT page fault
   → Write goes directly to page P, bypassing COW
   → P's refcount is 2 but parent just wrote to it
   → Child also maps P → corruption
```

After the child's COW-copy and exit, P's refcount drops to 1 while
the parent still maps P. The page allocator can then reuse P for a
different process — the phantom refcount scenario from Blog 146.

## The fix

Allocate a **fresh PCID** for the parent after fork:

```rust
pub fn duplicate_from(original: &mut PageTable) -> ... {
    let new_pml4 = duplicate_table_cow(original.pml4, 4)?;
    // Stale WRITABLE entries on remote CPUs are tagged with the
    // OLD PCID. Give the parent a new PCID so they're never used.
    if original.pcid != 0 {
        original.pcid = alloc_pcid();
    }
    // Flush current CPU with the new PCID.
    unsafe {
        let flush_cr3 = original.pml4.value() as u64 | (original.pcid as u64);
        x86::controlregs::cr3_write(flush_cr3);
    }
    Ok(PageTable { pml4: new_pml4, pcid: alloc_pcid() })
}
```

### Why this works

- **Current CPU**: CR3 write without bit 63 flushes all entries for
  the new PCID. Since it's fresh, there are no entries to flush, but
  future accesses will walk the page table and see read-only PTEs.

- **Remote CPUs**: Their stale entries are tagged with the OLD PCID.
  When the parent is next scheduled on a remote CPU, `switch()` loads
  CR3 with the NEW PCID (no-invalidate). The TLB has zero entries
  for the new PCID, so all accesses page-walk and see correct PTEs.

- **Without PCID** (pcid==0): every context switch does `cr3_write`
  without bit 63, fully flushing the TLB. Stale entries can't persist
  across switches, so no fix is needed.

### Cost

One `alloc_pcid()` call per fork — an `AtomicU16::fetch_add` (~1ns).
No IPI, no remote flush, no TLB pressure on other CPUs. This is
strictly cheaper than the alternative (TLB shootdown IPI to all CPUs).

## Secondary fix: device memory munmap panic

`munmap` on framebuffer pages (PCI BAR at 0xFD000000) panicked with
"invalid page address" because:

1. `map_user_page_with_prot` called `page_ref_init` on the device
   address, setting refcount=1
2. `munmap` called `page_ref_dec` → refcount dropped to 0 → `true`
3. `free_pages(0xFD000000)` → panic (not a managed RAM page)

**Fix:** Added `map_device_page()` that maps PCI device pages without
touching refcounts. Added `is_managed_page()` to the page allocator
so munmap skips free for device memory addresses.

## Secondary fix: #DB exception handler

Added a proper `#DB` (debug exception, vector 1) handler that reads
DR6 to identify which hardware watchpoint triggered, logs the faulting
RIP, disables the watchpoint to prevent infinite loops, and delivers
SIGTRAP for user-mode debug exceptions. Previously this panicked with
"unsupported exception: DEBUG".

## Cleanup

- Removed diagnostic COW tracing (per-fault `warn!` for stack pages)
- Removed hardware watchpoint setup code from the COW sole-owner path
- Removed refcount assertion from `duplicate_table` (was for diagnosis)
- Changed `Vm::fork(&self)` to `Vm::fork(&mut self)` to allow parent
  page table mutation (PCID reallocation)

## Additional fixes

**FBIOPUTCMAP/FBIOGETCMAP/FBIOPAN_DISPLAY ioctls**: Added no-op stubs
for colormap and pan-display ioctls on the framebuffer device. Xorg's
fbdev driver calls `FBIOPUTCMAP` 318 times during initialization on
TrueColor (32bpp) framebuffers. The kernel returned ENOTTY, causing
Xorg to log `(EE) FBDEV(0): FBIOPUTCMAP: Not a tty` for each call.
Now returns success silently.

**Missing test packages**: Added `xsetroot` and `xprop` to the Alpine
XFCE image builder. The `xsetroot_color` test was failing because
neither tool was installed.

## Test results

- BusyBox SMP 4: ALL PASS (29/29)
- Alpine smoke test: 56/56 PASS, 0 FAIL, 0 panics
- XFCE test: **8/8 PASS** (mount, fb0, fb_ioctl, dbus, xorg, xdpyinfo, xsetroot, xterm)
- No phantom refcount crashes across multiple runs

## Additional fix: IDT DPL for breakpoint vector

All 256 IDT entries had `DPL=0` (`info = 0x8E`). When userspace
executed `int3` (0xCC) — e.g., musl's `__stack_chk_fail` calling
`__builtin_trap()` — the CPU generated #GP instead of #BP, because
`DPL(0) < CPL(3)`.

**Fix:** Set `DPL=3` for vectors 3 (#BP) and 4 (#OF):
```rust
idt[i].info = match i {
    3 => 0xee, // #BP — DPL=3 for int3 from userspace
    4 => 0xee, // #OF — DPL=3 for into from userspace
    _ => 0x8e, // All others: DPL=0
};
```

Also changed the BREAKPOINT handler from `panic!` to properly deliver
SIGTRAP for user-mode `int3` instructions.

## Remaining issues

- XFCE session components crash with `call [rbp+0x40]` → NULL (Xlib
  extension `close_display` callback not populated). Client-side X
  extension library issue, not kernel. Components start but die during
  cleanup.
- Stack below VMA: PID 20 (Xorg) page faults at `0x9fffdefa0` — stack
  access below the mapped stack area. Needs stack guard page or stack
  auto-growth.
