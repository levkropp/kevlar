# 243 — Found it: Xorg loops in libshadow's framebuffer update

The non-stale PC sampler from blog 242 paid off.  Xorg, during the
phase-19 hang, is on cpu=1 with `stale_irqs=0` every sample —
genuinely running.  And the data is rock-stable across 5 seconds:

```
PID1_STALL: cpu=1 stale_irqs=0 user_pc=0xa0024b4b8 lr=0xa1bc6bf00
            x0=0xa1be0b7fc x1=0xa1c17feec x2=0x28
PID1_STALL: cpu=1 insns near lr-8: a9030fe4 97fff7fd b9402be5 a9430fe4
```

## Decoding the call site

`97fff7fd` at LR-4 = `bl <-0x200c>` → target offset `0xef0` in the
calling library.  Reading `/proc/4/maps` from the still-hung Xorg
(via `dd` to bypass procfs's 4KB read truncation), LR falls in:

```
a1bc69000-a1bc88000 r-xp fo=0x0          (124KB code segment)
```

A sweep of all candidate libraries on disk for the byte-pattern
`97fff7fd` at offset 0x2efc with `memcpy@plt` at 0xef0 yields
exactly one match: **`/usr/lib/xorg/modules/libshadow.so`**.

`objdump -d` puts LR-4 inside `shadowUpdatePacked + 0x214`:

```
2ed4: add  x1, x0, x2          ; x1 = dst + n  (overlap-check)
2ed8: cmp  x10, x1             ; src vs dst-end
2edc: b.hs 0x2ef0              ; if src >= dst+n, OK to memcpy
2ee0: brk  #0x3e8              ; OTHERWISE TRAP (compiler-emitted)
2eec: b    0x2edc              ; (dead code)
2ef0: mov  x1, x10
...
2efc: bl   memcpy              ; ← LR-4
2f00:                          ; ← LR
```

## What the args mean

- `x0 = 0xa1be0b7fc` (dst) — inside `a1bc8c000-a1bf8c000 rw-p` (3 MB)
- `x1 = 0xa1c17feec` (src) — inside `a1c000000-a1c301000 rw-p` (3 MB)
- `x2 = 0x28 = 40 bytes` — exactly 10 pixels at 32 bpp

**The dst region is exactly 3 MB at 1024×768×4 = framebuffer size.**
That mapping is `/dev/fb0` (ramfb).  The src is the matching shadow
buffer.  10 pixels = roughly one horizontal stripe of a cursor
sprite, copied via the dirty-region update loop.

So Xorg's `shadowUpdatePacked` is copying the same 40-byte cursor
row from shadow to framebuffer over and over, never returning to
`epoll_pwait`.  The damage tracker must keep reporting "still
dirty" without ever advancing past this row.

## The (red herring) panic

Logs include a kernel panic at `kernel/mm/page_fault.rs:668` —
`unwrap` on a `None` `current.vm()`, with `MAP_USER pid=0
vaddr=0xa1be0b000` immediately before.  Page address matches the
memcpy dst page.  Tempting to call this The Bug, but the panic
fires *at the end of the test* during cleanup, long after
`PID1_STALL` has been reporting the same loop for seconds.  It's
fallout from how the dying process's address space gets torn down;
not the trigger.

## Why it loops on Kevlar but not on Linux

This is the open question.  Possibilities:

1. **Damage region not cleared after copy.**  shadowUpdatePacked
   reads the damage list, copies, and is supposed to advance/clear
   it.  If the clear path requires an ioctl/write that Kevlar's
   ramfb silently drops, the region stays dirty forever.
2. **Damage feedback loop.**  Writing to `/dev/fb0` somehow
   re-marks the framebuffer dirty in the X server's bookkeeping.
3. **Cursor sprite re-render trigger.**  Xorg expects an interrupt
   or event to mark "cursor done"; absent that, it keeps re-running
   the sprite update.

Next move: instrument the damage tracker at the kbox/Xorg
boundary, or build a tiny reproducer that mmaps `/dev/fb0`,
writes pixels, and sees whether the matching damage event clears.

## Status

- Phase 19 reproducer: ✅ in-tree, 30 LoC of Rust
- Loop location: ✅ `libshadow.so::shadowUpdatePacked` calling memcpy
- memcpy args: ✅ dst=ramfb+0x17f7fc, src=shadow+similar, n=40 B
- Trigger: open — why doesn't the damage region clear on Kevlar?

The PC sampler (blog 242 → 243) is the most useful new debugging
primitive in this whole branch — it converts "Xorg hangs" into
"Xorg is at *this exact instruction in this exact library calling
memcpy with these exact args*" in one test run.
