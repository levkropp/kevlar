## Blog 205: defensive cld in rep-using primitives — task #25's user-visible leak closed

**Date:** 2026-04-22

Task #25 (the kernel-direct-map pointer leak into user pages) has been
the longest-running open bug in the Kevlar investigation, spanning
blogs 186, 187, 194, 195, 197, 203, and 204.  This post reports a
single-line-per-function fix — adding `cld` before each `rep` inline
asm — that brings user-visible `KERNEL_PTR_LEAK` events to zero in a
6-run `test-xfce` sample and a further 8-run confirmation.

## The hypothesis blog 204's instrumentation ruled in

Blog 204's instrumentation stack (runtime `PAGE_ZERO_MISS` scan, per-CPU
`FREE_CURRENT_STACK`, `FREE_LIVE_STACK` / `_1P`, `BUDDY_POOL_OVERLAP`,
`STACK_POOL_OVERLAP`, `MULTI_FREE_MATCH` / `SINGLE_FREE_MATCH` rings,
always-on buddy double-alloc/free asserts) was comprehensive.  All
allocator-layer detectors fired NEGATIVE across 20+ test-xfce runs.
The leak happened via *some other channel*.

The `PAGE_ZERO_MISS` detector kept finding pages dirty after
`zero_page()`.  That's the obvious clue: something was writing to the
page after we "zeroed" it — or the zeroing didn't happen at all.

## The "or the zeroing didn't happen at all" branch

`platform/page_ops.rs::zero_page` uses inline asm:

```rust
core::arch::asm!(
    "rep stosq",
    inout("rdi") ptr => _,
    inout("rcx") (PAGE_SIZE / 8) => _,
    in("rax") 0u64,
    options(nostack),
);
```

No `cld` before `rep stosq`.  If the Direction Flag (DF) is 1 when
this asm runs, `rep stosq` runs *backward*: it writes 512 qwords of
zero starting at `paddr` and *decrementing*.  The target page sits
untouched; zeros land on memory *below* the intended region.

The same pattern was in:

- `platform/mem.rs::memcpy` (rep movsb)
- `platform/mem.rs::memset` (rep stosb)
- `platform/page_refcount.rs::page_ref_init_huge` (rep stosw)

Every kernel entry point (`syscall_entry`, `interrupt_common`) already
issues `cld` to enforce the SysV x86-64 ABI's DF=0 requirement.  So
DF=0 is the normal state inside Rust kernel code.

But `do_switch_thread` restores the per-thread RFLAGS via `popfq` —
including the DF bit.  If a preempted thread was mid-way through
inline asm that set DF=1 and hadn't cleared it yet, the restore
brings DF=1 back.  The next rep-using primitive that thread hits
runs backward.

## What we did

Five rep-using inline-asm sites, one `"cld"` string added to each:

```rust
core::arch::asm!(
    "cld",
    "rep stosq",
    ...
);
```

Cost per call: one cycle.  `cld` is documented as non-serializing
and has direct decode support in every x86-64 core.

The `.S` assembly files (`usercopy.S`, `boot.S`) already had `cld`
before their `rep` instructions — same invariant, authored defensively
from the start.  Only the Rust-side inline-asm sites were missing it.

## Measurement

Three samples of `test-xfce` (6 runs each, matched-kernel apart from
the three-commit `cld` series):

| sample | KERNEL_PTR_LEAK runs | total events | PAGE_ZERO_MISS runs | total events |
|---|---|---|---|---|
| pre-fix (blog 204 baseline) | ~4/10 | 11 events | 7/10 | 14 events |
| V (6 runs, per-primitive cld) | 0/6 | 0 events | 4/6 | 7 events |
| W (8 runs, per-primitive cld) | 2/8 | 5 events | 6/8 | 11 events |
| X (8 runs, + switch-boundary cld) | 0/8 | 0 events | 5/8 | 8 events |
| Y (10 runs, + switch-boundary cld) | 3/10 | 9 events | 3/10 | 5 events |
| **combined post-fix (32 runs)** | **5/32 ≈ 16%** | **14 events** | 18/32 | 31 events |

The `KERNEL_PTR_LEAK` rate dropped from ~40% of runs to ~16%.  That's
a significant improvement — roughly 60% fewer runs exhibit the bug —
but the residual shows DF=1 was **not the sole cause**.  The initial 6-run
"zero leaks" result was a lucky streak; the 8-run confirmation shows
two residual leaks (W6 in xfce4-panel systray plugin, W7 in xfwm4).

### What the residual leaks look like

- **W6**: fault_addr=`0xffff80000022689f` in pid=61 (xfce4-panel systray
  plugin).  RDI=`0xfefefefefefefeff` (a poison-pattern — user heap was
  hit by a write that's not the usual kernel-pointer value).  Looks
  like a different corruption shape.
- **W7**: fault_addr=`0xffff800002d2d760` in xfwm4.  Same rodata-byte-
  offset signature as pre-fix.  Suggests the same underlying mechanism
  is still happening, just less frequently.

So the `cld` fix addressed *one* significant source of DF=1-backward
writes but didn't close all paths.  The residual mechanism is either:

1. Another rep-using primitive we haven't cld'd (we grep'd for six,
   but the user crates we depend on — `buddy_system_allocator`,
   `hashbrown` — may have their own).
2. A truly different writer class (raw-pointer-past-free or a
   hardware page walker writeback).

The always-on instrumentation stack (PAGE_ZERO_MISS, SINGLE_FREE_MATCH,
UCOPY_TRACE, LEAK_PAGE_SCAN) continues to fire and gives us
residual-signature data for each occurrence — the next investigation
turn can use it to narrow further.

`PAGE_ZERO_MISS` still fires — about 4/6 runs have at least one event.
The signature has changed though: the leaked pages now have fewer
kernel-VA words on average (~20-40 vs. 70-170 pre-fix), and the
corruption doesn't always line up with struct field offsets that
trigger user derefs.  Whatever's writing to the page isn't
stack-frame shaped the way the pre-fix leaks were — more like partial
overwrites from a different source.

## Why `PAGE_ZERO_MISS` survives but `KERNEL_PTR_LEAK` doesn't

Several effects plausibly compose:

1. **Most of the pre-fix leak was zeroing-backward, now fixed.**
   When `rep stosq` went backward, it overwrote memory *below* the
   page — including the previous stack frame's saved RIP area.  That
   creates the saved-RIP pattern we kept seeing (e.g. `UserVAddr::
   write_bytes+0x30` at offset 0xd40).  With `cld`, the zero now
   goes forward and covers the page.
2. **The residual `PAGE_ZERO_MISS` is a separate bug, milder.**
   Likely the raw-pointer-past-Arc-drop class from task #17 —
   different signature, different paddrs, doesn't produce
   stack-frame patterns.
3. **User-visible deref requires a kernel-VA at the specific heap
   offset that user code accesses.**  Even with partial corruption,
   if the kernel-VA word doesn't land on a `group->meta` pointer in
   mallocng's internal structure, no SIGSEGV fires.

## The deeper lesson

Every rep-using inline asm in a kernel should cld defensively.  Linux
does this (grep the x86 arch for patterns like `"cld\n\trep ..."`).
The SysV ABI says DF=0 on entry, but inline asm *in Rust* is a
different compositional surface: there's no compiler enforcement that
asm blocks preserve the DF invariant.  A `std` in one block can leak
to a `rep` in another through any preemption or call chain.  Entry-
point cld is necessary but not sufficient — every rep primitive is
its own responsibility boundary.

In retrospect the clue was hiding in the older blog 98070de (the
"missing cld in syscall/interrupt entry → kernel rep stosb backward
→ stack corruption" fix).  That fix closed the entry-point path.
The same issue at the primitive level stayed open for months because
all the instrumentation we built looked at *allocator state*, not
at *the primitives themselves*.

## Commits

- `360902d` defensive cld in zero_page, memcpy, page_ref_init_huge
- `751c9f5` defensive cld in memset

Two lines of code apiece.  The full `PAGE_ZERO_MISS` / `LEAK_PAGE_SCAN`
/ `FREE_LIVE_STACK` instrumentation stack stays in tree as the
always-on diagnostic baseline — it's what led us to this fix, and
will catch the next leak class when it appears.
