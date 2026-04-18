## Blog 186: a kernel pointer in a user heap, and what it means

**Date:** 2026-04-19

After the fixes in [183](183-stale-cr3-pt-cookie.md),
[184](184-poll-pollnval.md), and [185](185-pixbuf-builtin-png.md) the
10-run test-xfce sample was stable enough to read the remaining
xfce4-session SIGSEGVs carefully.  Run 10 gave the most informative
one:

```
SIGSEGV: no VMA for address 0xffff80003f857a40
  (pid=20, ip=0xa0007cacd, reason=PRESENT | CAUSED_BY_USER)
  RDI=0xa1fd652d0 RSI=0xf RBP=0xa1f097af0
  RAX=0xffff80003f857a30
  code at ip=0xa0007cacd: 48 39 48 10 74 01 f4 0f b6 48 20 ...
```

RAX is `0xffff80003f857a30`.  That is *exactly* the kernel's
direct-physical-map base `KERNEL_BASE_ADDR = 0xffff_8000_0000_0000`
(see `platform/x64/mod.rs:253`) plus physical address `0x3f857a30`.
A userspace process is holding a pointer into kernel memory.

## Where it crashed

The crash IP's VMA is `[0xa0004b000-0xa000a2000]`, file size `0x56e01`,
file offset `0x14000`.  That RX size+offset matches *exactly* one file
in the rootfs:

```
readelf -l ld-musl-x86_64.so.1 | grep -A1 LOAD
  LOAD  0x0000000000014000 ... 0x0000000000056e01 R E  0x1000
```

So the crash is inside musl's dynamic linker, not in any XFCE code.
File offset `0x45acd` disassembles to:

```
45abb:  mov   eax, esi          ; slot index
45abd:  shl   eax, 4            ; * 16
45ac0:  cdqe
45ac2:  sub   rdi, rax           ; rdi = group base
45ac5:  mov   rax, [rdi-0x10]   ; rax = group->meta
45ac9:  lea   rcx, [rdi-0x10]
45acd:  cmp   [rax+0x10], rcx   ; ← faults. meta->mem must = &group->meta
45ad1:  je    45ad4
45ad3:  hlt                      ; unreachable if cmp passes
```

This is musl `mallocng`'s free-path consistency check.  Every chunk
has a 4-byte header above it that identifies which slot of which
group it belongs to; the group header stores a `meta*`, and `meta`
stores a back-pointer to its group.  `free()` walks `chunk → group →
meta` and asserts `meta->mem == group`.

So `[rdi-0x10]` — the `meta*` field of the group header — contains
`0xffff80003f857a30`.  That's what should have been a musl-allocator
pointer.  It is, instead, a kernel direct-map pointer.

## What this tells us

This is not a logic bug in musl and not a logic bug in xfce4-session.
A user heap page had kernel data in it.  The only way that happens is
a broken page-transition boundary — kernel→user handoff that leaves
kernel bytes visible.

Kevlar's page handoff is in `platform/page_allocator.rs`:

- `alloc_page(USER)` without `DIRTY_OK` → either pops a pre-zeroed
  page from `PREZEROED_4K_POOL`, or memsets on the path out
  (`page_allocator.rs:196-231`).  Pre-zeroed pages are explicitly
  zeroed before being pushed (`prefill_prezeroed_pages`,
  `refill_prezeroed_pages`).

- `alloc_page(USER | DIRTY_OK)` skips zeroing.  This flag is used in
  four places in `kernel/mm/page_fault.rs`; every one of them
  immediately does a full-page `dst.copy_from_slice(src)` before
  releasing the page back to userspace.  No fall-throughs.

So on the static alloc-path side, everything is zeroed.  That leaves
the *TLB side*: a user process holds a valid mapping for virtual page
V, but the physical page P that V points to has since been given to
the kernel, overwritten with kernel data, and handed back to userspace
at a different VA — while the original mapping V → P was never
invalidated on the CPU that still holds it.  The TLB walker serves
reads and writes against P's current content, which is kernel data.

This is exactly the failure mode from [blog 175][175] (pre-fix): COW
freed the old page without a multi-CPU shootdown, a remote CPU's stale
TLB entry still resolved to the recycled physical page, and the process
saw whichever data happened to land there.  The fix at
`page_fault.rs:837` changed that `flush_tlb_local` → `flush_tlb` (all
CPUs).  But run 10 shows a symptom of the same shape, so either:

1. There's a different CoW path we missed,
2. A non-CoW path also frees user pages without a cross-CPU flush,
3. `flush_tlb` itself isn't reaching all CPUs when called under
   certain interrupt-context conditions.

All three are task #25 territory.  This crash doesn't pinpoint which
— but it does *confirm* that pointers reachable only from the kernel
direct map are landing in user heap pages, which is the cleanest
possible evidence that a stale user PTE is seeing recycled kernel
memory.  Previous task #25 evidence was shaped as "code page contains
string bytes" or "GOT entry points at wrong function"; this one adds
"heap header contains kernel-VA pointer."

[175]: 175-xfce4-session-segv-deep.md

## The Xorg sibling

The 1/10 Xorg SIGSEGV in run 8 has the same *assertion* shape but not
the same *root cause signature*:

```
USER FAULT: GENERAL_PROTECTION_FAULT pid=8 ip=0xa00254c92
verify_text: ip=... — NO DIFF
code: f4 80 7a fb 00 0f 84 6d ff ff ff f4 ...
```

`f4` = `hlt`.  Executing `hlt` in ring 3 is a GPF.  The crash IP is
*on* a hlt, and the `verify_text` probe — which re-reads the crash
page from the backing ELF and diffs — reports `NO DIFF`, so this hlt
is genuinely part of the compiled Xorg binary.  Like the musl case,
it's an "unreachable after assertion" marker that a prior branch
took wrong.  But there's no kernel pointer in the registers — just an
assertion failure from Xorg's internal state being wrong, which could
be page corruption *or* a plain logic bug.  Can't tell from a single
crash.

## Status of the XFCE-userspace session

After this session (blogs 183–186):

| blog | bug | fix site | outcome |
|---|---|---|---|
| 183 | stale CR3 → PT cookie corruption | kernel (`switch.rs`) | 10/10 clean kernels |
| 184 | poll(2) returned EBADF, not POLLNVAL | kernel (`poll.rs`) | xfce4-session 5/10 → 3/10 |
| 185 | loaders.cache missing builtin PNG/JPEG | rootfs (`build-alpine-xfce.py`) | Thunar 3/10 → 0, xfdesktop silent-respawn → 2/10 visible |
| 186 | (this one) — kernel pointer in musl heap | not fixed; diagnosed as task #25 | — |

The three fixable bugs landed.  The remaining crashes all look like
downstream symptoms of one open bug: text/heap pages containing
kernel data or wrong-process data because a stale TLB entry on some
CPU points at a recycled page.  Task #25's investigation continues.
