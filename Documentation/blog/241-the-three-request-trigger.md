# 241 — The three-request trigger

The openbox→Xorg hang has a 50-line repro now.  Three X11 requests,
no openbox installed, no captured byte trace required:

```
1. OpenFont(fid, "cursor")
2. CreateGlyphCursor(cid, source=fid, mask=fid, src_char=68, mask_char=69)
3. ChangeWindowAttributes(root, CW_CURSOR=cid)
```

Replaying just that triplet hangs `xprop` for 30s, the same hang
signature that's haunted task #41 since blog 233.  This is the
minimal kernel-bug reproducer.

## How we got here

Blog 240 captured the byte trace (139 C2S chunks, 4608 bytes) via
`kxproxy` and replayed it via `kxreplay` to confirm the hang
reproduces from bytes alone.  Three rounds of bisection narrowed
the trigger:

| Bisection step | Inputs | Result |
|---|---|---|
| All 139 chunks | full trace | HANG |
| `kxreplay-limit=138` | chunks 0..137 | NO HANG |
| `kxreplay-limit=138 tail=16` | chunks 0..136 + first 16B of chunk 137 | HANG |

Bytes 0..15 of chunk 137 = `02 00 04 00 42 00 00 00 00 40 00 00 02 00 20 00`
= `ChangeWindowAttributes(window=0x42, value_mask=CW_CURSOR=0x4000, cursor=0x00200002)`.

Then `KXREPLAY_INCLUDE` for non-contiguous chunk selection bisected
the *prior* context.  The minimum prior chunk is **#131**, a 484-
byte batch whose first request is `OpenFont(fid=0x00200001, "cursor")`
followed by `CreateGlyphCursor(cid=0x00200002, source=0x00200001, ...)`.
With `KXREPLAY_TAIL_IDX=131 KXREPLAY_TAIL=52` (chunk 131 cut to
just those two requests) and chunk 137's first 16 bytes, the hang
still reproduces.  Total trigger: **620 bytes** including the
12-byte handshake.

## Phase 19 — kbox built from scratch

`tools/kbox/src/wm.rs::phase19_minimal_cursor_trigger` issues
exactly those three requests:

```rust
let (_, fid) = open_font(&mut out, conn, "cursor");
let (_, cid) = create_glyph_cursor(&mut out, conn, fid, fid,
    68, 69, (0, 0, 0), (0xffff, 0xffff, 0xffff));
let _ = change_window_attrs_cursor(&mut out, conn, conn.info.root_xid, cid);
flush(&mut out, conn)?;
```

Run it with `KBOX_PHASE=19` and `xprop -root _NET_SUPPORTING_WM_CHECK`
hangs for 18-30s.  Same failure mode as full openbox.

## Ablation — all three are essential

`KBOX_PHASE_19_VARIANT` controls which subset runs:

| Variant | Sequence | Result |
|---|---|---|
| `all` | OpenFont + CreateGlyphCursor + CW_CURSOR | **HANG** |
| `noset` | OpenFont + CreateGlyphCursor (no CW_CURSOR) | pass |
| `noglyph` | OpenFont + CW_CURSOR with uncreated cid | pass |
| `nofont` | CreateGlyphCursor with junk font + CW_CURSOR | pass |

The trigger requires (a) a real font, (b) a real cursor created
from that font, and (c) installing it on root.  Any one missing
and Xorg behaves normally.

## What the kernel sees

Strace on Xorg (pid 4) during phase 19 shows the same pattern as
real openbox:

```
recvmsg(7) → 84 bytes      ← xprop's request batch
clock_gettime → 0
recvmsg(7) → -EAGAIN       ← drain check
setitimer
clock_gettime → 0
... <silence, no syscalls for 30s>
```

The TICK_HB heartbeats keep firing on both CPUs; Xorg's `pid1_gap`
grows to 232+ ticks.  That means the kernel isn't deadlocked —
Xorg is in a userspace tight loop, monopolizing CPU, never
returning from request processing to issue its `writev` reply.

## Why this matters

Three lines of well-formed X11.  No openbox.  No captured byte
trace.  No 1.5MB closed binary in the loop.  The trigger is now
in our source tree, in `phase19_minimal_cursor_trigger`, and the
test reproduces deterministically every run.

The next investigation is *what specifically about Kevlar's kernel
state* makes Xorg's cursor path infinite-loop where Linux's
doesn't.  Candidates: framebuffer mapping (ramfb), shared memory,
page-fault loop, font-glyph data corruption.  None of those are
syscall-visible, so the next move is either an arm64 stuck-process
PC dumper (read user PC during long pid1_gap stretches) or an
even smaller C reproducer that pinpoints which Xorg subsystem
loops.
