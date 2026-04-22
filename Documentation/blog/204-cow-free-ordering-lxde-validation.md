## Blog 204: CoW free-before-flush, LXDE validation, and the remaining leak surface

**Date:** 2026-04-22

Blog 203 landed the per-CPU PCID generation fix and measured 5-run
`test-xfce` at 2/5 fully leak-free.  The leak signature in the
remaining 3/5 runs changed shape: instead of the recurring `0x2d074d8`
and `0x22c9f7` paddrs that dominated pre-fix, each run had a unique
paddr and the same user-code context (musl mallocng's free-path
assertion at `ip=0xa00093acd`).  That's the signature of a narrower,
rarer race — not a different bug.

This post covers two follow-on fixes and the LXDE validation that
stress-tested them, then catalogs the instrumentation we need to build
next to close the residual leak surface.

## Fix 1: CoW frees before cross-CPU flush

Auditing `free_pages` callers for missing TLB flushes turned up three
sites in `kernel/mm/page_fault.rs`, all in CoW paths:

1. **Shared-read-to-writable upgrade** (refcount>1 or ghost, write to
   read-only shared page).  Old ordering:
   ```rust
   page_ref_dec(old_paddr);
   if page_ref_dec(old_paddr) { free_pages(old_paddr, 1); }
   map_user_page_with_prot(aligned_vaddr, new_paddr, prot_flags);
   flush_tlb(aligned_vaddr);
   ```
   The free happens *before* the cross-CPU flush.  A sibling thread
   on another CPU with `V→old_paddr` cached can write through the
   stale TLB entry at any point between `free_pages` and the IPI
   acknowledgment — landing the write on a page the allocator has
   already returned to its pool and possibly handed to another caller.

2. **Anonymous-CoW** (refcount>1, write to private anon page).  Same
   ordering bug, plus the flush was `flush_tlb_local` only.  When the
   fault-handling process is the sole owner across processes, sibling
   threads on other CPUs may still have `V→old_paddr` cached — a
   local-only flush leaves those translations live.

3. **RELR-write CoW** (MAP_PRIVATE file-backed, write to read-only
   page — used by musl's RELR relocation processing).  Same pattern.

Fix in all three: compute `should_free_old` from the refcount-dec
result but defer the actual `free_pages` until after `flush_tlb`.  For
(2) and (3), upgrade `flush_tlb_local` → `flush_tlb` (cross-CPU)
*only* when we're about to free; unchanged in the refcount>0 branch
since the page isn't going back to the allocator.

Committed as `e5e287d`.  Regression-tested:

- `test-threads-smp`: 14/14 (unchanged)
- `test-tlb-stress`: 696ms, down from 713ms — the cross-CPU flush
  cost is a wash with the cache-coherency win from freeing after the
  flush completes
- `test-xfce` (5 runs): 2/5 leak-free, 3/5 with one residual leak each

## LXDE validation

Task #6 was to try a different desktop environment as a counter-
example — maybe XFCE's specific failure mode (xfce4-session's D-Bus/
ICE spawn handshake taking 48+ seconds) masked other issues, or the
XSM protocol itself was an unstable surface.

Alpine 3.21 doesn't ship the original LXDE (lxsession/lxpanel), but
it does ship the three independent binaries that LXDE-on-Alpine users
typically combine: openbox (window manager), tint2 (panel), and
pcmanfm (file manager in `--desktop` mode for wallpaper + icons).
No session manager, no XSM handshake — each component is started
directly and owns its own lifecycle.

New infrastructure:

- `tools/build-alpine-lxde.py`: apk-installs the stack + writes an
  openbox autostart + a pcmanfm `wallpaper_mode=color` config
- `testing/test_lxde.c`: 6 tests (mount, Xorg, openbox, tint2,
  pcmanfm, pixel visibility), launches components directly rather
  than via openbox-session (openbox-xdg-autostart is a separate
  missing package on Alpine)
- `make test-lxde` + `make run-alpine-lxde`

5-run `test-lxde` on the fixed kernel:

| Run | Score | Leaks | Notes |
|---|---|---|---|
| 1 | 6/6 | 0 | fully clean |
| 2 | 6/6 | 0 | fully clean |
| 3 | 5 incomplete | 1 event | tint2 SIGSEGV (KERNEL_PTR_LEAK) |
| 4 | 5 incomplete | 0 | kernel panic: rip=0 in lockdep::on_acquire |
| 5 | 5/6 | 0 | pcmanfm overdrew xsetroot (test-harness flake) |

LXDE exposes the *same* two kernel bug classes as XFCE:

1. **KERNEL_PTR_LEAK** — now observed in tint2 (a plain C program),
   not just musl's mallocng assertion.  The bug class is independent
   of the specific user-code shape.
2. **rip=0 lockdep::on_acquire panic** — seen in 1 LXDE and 1 XFCE
   run, same backtrace each time.

That's actually useful negative information: the shape of the
remaining leaks isn't specific to XFCE's startup topology.  Whatever
the kernel is doing wrong, it's about page recycling, not about any
particular user-space workload.

## The remaining surface

Two bug classes survive the fixes through blog 203:

### KERNEL_PTR_LEAK (task #25)

8-byte kernel direct-map pointer `0xffff_8000_XXXXXXXX` shows up in
user heap memory.  User code loads it into a GPR, dereferences, SIGSEGV.
Post-fix paddr distribution:

| paddr region | hypothesis |
|---|---|
| `~0x21e000` .text | kernel stack frame, recently freed |
| `~0x2cfe000` .rodata | global kernel data, freshly recycled |
| `~0x3a000000` heap | slab page, recycled from kernel user |

The fix-stack-to-date covers every identified call-site race:
- broad-sti in syscall_entry (blog 198)
- per-CPU PCID generation (blog 203)
- CoW free-before-flush (this blog)
- always-defer Vm::Drop with QSC grace period (blog 195)

No remaining call site has been identified as a race.  Either one of
the existing audits missed a path, or the bug is more subtle than
"missing TLB flush."

### rip=0 lockdep panic

CPU enters idle, wakes from an interrupt, context-switches through a
few processes, enters idle again, then panics with `rip=0 vaddr=0` —
meaning the CPU tried to fetch an instruction from address 0.
Backtrace shows `lockdep::on_acquire` and `interrupt_common`.

This has the shape of kernel-stack corruption during idle — someone
wrote to the idle thread's kernel stack where a return address was
stored, so `ret` popped 0 off the stack.  Blog 196 documented the
"eager stack release" pattern that could do this; it was supposedly
fixed in `26cb5d2`.  Either there's another eager-free path, or
something else is stomping the stack.

## Instrumentation we need to build

The existing leak detector tells us *that* a leak happened, and
*which* paddr leaked, but not *how* it got there.  To close this bug
class we need:

### 1. Page-content provenance (highest priority)

When `alloc_page(USER)` returns paddr P, the caller assumes P is
zero-filled.  The existing `debug_assert_page_is_zero` scans P for
non-zero bytes — but it's compiled out of release builds, so we don't
run it during the XFCE/LXDE tests where the bug actually shows up.

Need: **runtime-enabled zero-fill check that's cheap enough to leave
on**, plus **kernel-VA-shape detection** (count 8-byte words matching
`0xffff_8000_XXXXXXXX`).  If a freshly-allocated user page contains
kernel-VA-shaped values before user code has written to it, that's a
smoking gun that memset didn't cover the page, or that something
wrote to the page between memset and return.

### 2. Leak-context page scan (second priority)

When a `KERNEL_PTR_LEAK` fires, scan the *entire user page* at the
faulting VA.  Count:
- total 8-byte kernel-VA-shaped values
- their offset distribution (clustered = real kernel data structure;
  scattered = coincidental byte pattern)
- paddr distribution (.text region? heap? stack-shaped?)

This turns "one pointer leaked" into "the page has N kernel pointers
at these offsets" — enough context to identify what kernel data
structure ended up in the user page.

### 3. Paddr history ring buffer (third priority)

For each paddr in a bounded range (tracked by `kevlar_platform::
page_refcount` already), record a small ring buffer of events:
alloc-for-user, alloc-for-kernel, free-to-user-cache, free-to-kernel,
last-kernel-write-via-direct-map.  On `KERNEL_PTR_LEAK`, dump the
ring buffer entries for the leaked paddr.

This is more expensive (memory + per-alloc/free work) but gives a
direct answer: "paddr P was last used by the kernel slab for
Arc<Process>, freed at TSC=X, handed to user at TSC=Y (delta 50µs),
and written via direct map at TSC=Z (between free and user-alloc) —
kernel code at RIP=..." The direct-map write between free and
user-alloc is the exact bug we're hunting.

### 4. TLB coherency validator (fourth priority)

Periodically (every ~100ms or at syscall entry), sample a random
user-mapped paddr from the current process's page table and verify
that no other online CPU has a TLB translation for a different VA
to the same paddr *unless* that VA is also in some process's
page table.  Hard to implement on x86 (no architectural TLB dump)
but can be approximated via `INVLPG`-miss-counting: if invlpg of an
unmapped VA doesn't flush a lurking translation, the hardware walker
is doing something we don't expect.

## Priorities

Items 1 and 2 are implementable in ~100 lines each and run on every
XFCE/LXDE boot without meaningful overhead.  They'd tell us within
a few runs whether the bug is zero-fill-on-alloc or a write-after-free.

Item 3 is maybe 300 lines and costs ~200 bytes per physical page
(reasonable for systems up to ~16GB; we're running at 1GB in tests).
It's the "definitely finds the bug" tool if item 2 doesn't narrow
it down.

Item 4 is speculative and a day of work.  Skip unless 1-3 miss.

## Built this turn: items 1 and 2

### Runtime zero-fill check with kernel-VA detection

`platform/page_allocator.rs::debug_assert_page_is_zero` was
`#[cfg(debug_assertions)]`-gated.  Changed to:
- Runtime-toggleable via `PAGE_ZERO_CHECK_ENABLED` atomic (default on)
- Runs in release builds
- Counts misses via two atomics: `PAGE_ZERO_MISS_COUNT` and
  `PAGE_ZERO_MISS_WITH_KERNEL_PTR`
- Logs first `PAGE_ZERO_MISS_LOG_LIMIT = 32` misses with full detail
  (site, paddr, first nonzero offset, count, kernel-VA word if any)
- Silent-counts the rest so a leaky run is visible in the summary
  without flooding serial output

New log line shape on hit:

```
PAGE_ZERO_MISS site=PAGE_CACHE_memset paddr=0x3a083000 first_nz_off=0x88 \
  nonzero_words=12 kernel_ptr_words=1 (seen #3)
  first kernel-VA word: paddr+0x00088 = 0xffff80003d1e5210 (target paddr=0x3d1e5210)
  +0x060: 0x0000000000000000
  +0x068: 0x0000000000000000
  ...
  +0x088: 0xffff80003d1e5210 <<<
  ...
```

If *any* fresh user page has kernel-VA-shaped words in it, this line
will fire on alloc — *before* user code has had a chance to touch the
page — giving us a direct answer to "did the page come back dirty?"

### Leak-context page scan on KERNEL_PTR_LEAK

`kernel/mm/page_fault.rs::scan_user_page_for_kernel_ptrs` scans the
4KB user page at RDI/RBP/RSI (the registers most likely to hold a
pointer to the user memory from which the leaked kernel pointer was
loaded).  Reports:
- total kernel-VA 8-byte words in the page
- paddr distribution classified as kernel-image / kernel-heap / mixed
- first 8 offsets and values

New log line shape:

```
LEAK_PAGE_SCAN RDI: vaddr=0xa1f1fd000 paddr=0x25d8b000 kernel_ptrs=47 \
  in kernel-heap region (page_size=4096)
  first kernel-VA at +0x018 = 0xffff80003d1e5210 (target paddr=0x3d1e5210)
  [+0x018] = 0xffff80003d1e5210
  [+0x028] = 0xffff80003d200000
  [+0x030] = 0xffff80003d204830
  ...
```

kernel_ptrs=47 in a user page means the page almost certainly is
serving a stale TLB translation to a kernel data structure.
kernel_ptrs=1 would mean it's a single coincidental value in an
otherwise-empty page.  That distinction closes the hypothesis gap
between "page-recycle missing flush" vs "single pointer crossed a
boundary somewhere."

### What this should tell us

Running `test-xfce` and `test-lxde` with these enabled, one of three
results lands:

1. **PAGE_ZERO_MISS fires with kernel-VA words** — the bug is in
   the page allocator's zero-fill path (memset isn't covering the
   page, or something writes to the page between memset and return).
2. **PAGE_ZERO_MISS never fires, but LEAK_PAGE_SCAN shows density=many** —
   alloc-time page is clean but the user VA's PTE later resolves to
   a different paddr that holds kernel data.  The TLB on the faulting
   CPU has a stale translation from some earlier owner.
3. **PAGE_ZERO_MISS never fires, LEAK_PAGE_SCAN shows density=1** —
   one specific 8-byte value made it into user memory through a
   non-TLB channel (direct kernel write via copy_to_user, shared-
   mapping mismatch, etc.).

Each outcome points at a disjoint fix path.  Committing and measuring
next turn.
