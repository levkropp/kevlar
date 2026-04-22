## Blog 206: Session summary — instrumentation-driven bug hunting and the cld fix

**Date:** 2026-04-22

This post wraps up a long session on task #25 (the kernel-direct-map
pointer leak into user pages), the LXDE validation work, and
establishes a handoff state for the next session.  It's part
retrospective, part handoff.

## What landed

**Kernel fixes (task #25 partial closure, ~60% reduction):**

- `a6cbbe8` PCID per-CPU last-seen generation tracking — closes a
  PCID-recycle TLB hole (blog 203)
- `e5e287d` CoW free-before-flush + cross-CPU scope in three sites
  in `page_fault.rs`
- `360902d`, `751c9f5` Defensive `cld` in zero_page, memcpy, memset,
  page_ref_init_huge — each is a one-line addition
- `7ae6f15` `cld` after `popfq` in `do_switch_thread` — enforces DF=0
  at every context-switch boundary, the closest thing to a global
  DF=0 invariant a Rust kernel can have

**Measurement:**

| sample | KERNEL_PTR_LEAK rate |
|---|---|
| Pre-fix (blog 204 baseline) | ~40% of XFCE runs |
| Post-cld (combined 32 runs) | ~16% |

The 60% reduction is large enough that user-visible SIGSEGVs in XFCE
are materially less frequent.  The residual 16% is a *different* bug
class that happens without a corresponding PAGE_ZERO_MISS — corruption
after the page is handed to the user, not at zeroing time.

**LXDE validation (blog 204 task #6 closed):**

- `d886eca` Alpine-LXDE-style test: openbox + tint2 + pcmanfm stack.
  Alpine 3.21 doesn't ship the original LXDE, but the openbox+tint2+
  pcmanfm combination gives us a second graphical workload to cross-
  check XFCE findings against.  First run: 6/6 PASS.  Same task-#25
  leak surface as XFCE.

**Instrumentation stack (all always-on, released to tree):**

Eleven detectors + a Python summarizer.  The full list is in
`project_leak_instrumentation.md` but the key ones:

- `PAGE_ZERO_MISS` — runtime zero-fill check + kernel-VA density
- `LEAK_PAGE_SCAN` — on KERNEL_PTR_LEAK, scans user pages for
  kernel-pointer density
- `FREE_CURRENT_STACK` — per-CPU registry of kernel / IST / syscall
  stack paddrs, updated on every context switch.  Panics if any of
  them is ever freed.
- `SINGLE_FREE_MATCH` + `MULTI_FREE_MATCH` — ring buffers of recent
  frees with caller RIP captured via RBP walk
- `UCOPY_TRACE` — assembly-level ring buffer of every copy_to_user /
  copy_from_user with (dst, src, len, ret_addr)
- Buddy allocator's `debug_assert!` double-alloc/free checks converted
  to always-on `assert!` (commit `3edaab6`).  This was a critical
  discovery: the checks were compiled out of release builds, so we'd
  been running blind to buddy state corruption for the entire task #25
  investigation.
- `tools/analyze-leak-log.py` — summarizer that slurps test-xfce or
  test-lxde logs into a one-screen report

## The lesson that matters most

The task #25 investigation burned through ~8 wrong hypotheses before
landing on DF=1.  Every one of those wrong hypotheses was
*allocator-level*: double-free, stack-paddr-overlap, PCID recycle,
CoW free ordering, Arc-past-drop, stale TLB.  Each hypothesis got
its own detector; each detector fired NEGATIVE.

The actual bug was the x86 primitive layer: `rep stosq` running
backward because DF=1 leaked past an entry-point `cld` via
`do_switch_thread`'s `popfq`.  Every Rust kernel relies on the SysV
x86-64 ABI's DF=0 invariant, but ABIs aren't enforced at the
inline-asm boundary — if some code path sets DF=1 and gets
preempted, the next context-switch boundary (`popfq`) restores DF=1,
and the next rep primitive runs backward.

The fix is *trivial* — one `cld` instruction at each boundary.  It
took weeks to find because all the instrumentation was pointed at
the allocator.  In retrospect, blog 186 should have been the first
clue: the leaked pointer `0xffff80003f857a30` was at offset 0xd40 in
a user page, which is exactly where the PREVIOUS stack frame's saved
RIP would land if `rep stosq` went backward from the target paddr.
We saw the pattern every run and never matched it to direction.

Subtext for the next investigation: **when every allocator-level
detector is negative and corruption persists, consider primitive-
level bugs (DF, prefetcher ghost writes, cache coherence) before
inventing more allocator hypotheses.**

## What's next (the open tasks)

Five tasks remain in-pipeline:

| # | description | why it matters |
|---|---|---|
| 15 | Extend stack_cache::is_stack_paddr with recent-unregister window | Catches `unregister_stack → free → still running` races |
| 16 | PT-page-cookie corruption (blog 194 signature) | Still fires occasionally; separate class from task #25 |
| 18 | `syscall_stack=None` on scheduled task | Found in T2 run; implies Arc<Process> drop with a raw ptr surviving |
| 19 | Residual PAGE_ZERO_MISS shape analysis | Distinguish raw-ptr-past-free vs stale-TLB-after-munmap |
| 20 | Remaining raw-PAddr-holding kernel data structure | Where's the 16% residual leak coming from? |

The 16% residual in task #25 has the pre-fix signature (paddr=
`0x2d2d760` in xfwm4) but happens on runs where PAGE_ZERO_MISS
doesn't fire.  That means the page is clean at alloc time — some
other writer corrupts it after the user has it.  Task #19 with the
current instrumentation stack should close this.

## Handoff artifacts

- `Documentation/FUTURE_SESSION_ARM64_PROMPT.md` — a copy-pasteable
  prompt for the next Claude Code session, specifically for ARM64
  validation + resuming the graphical-desktop work
- `project_leak_instrumentation.md` — the instrumentation registry,
  kept up to date by the session that added each detector
- This blog post — the narrative summary

## Stats

- 25 commits this session (leak instrumentation + fixes + blogs)
- 3 blog posts (203 PCID, 204 CoW/LXDE, 205 cld fix, this one)
- 2 new test harnesses (test-lxde, test_tlb_stress)
- 11 always-on runtime leak detectors
- 1 Python analyzer script

The tooling is now the best part of the Kevlar codebase by a wide
margin.  Future bugs in this class will close in hours, not weeks.
