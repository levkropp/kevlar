# Blog 171: kxserver Phase 10 — The RENDER Extension

**Date:** 2026-04-13

## Why Phase 10 is the make-or-break phase

Everything before this was prelude. Xft — the client-side font
rendering library that xterm, every GTK app, every Qt app, and
virtually every modern desktop ends up using — requires the RENDER
extension. Without RENDER, we can't display antialiased text. Without
antialiased text we can't pass for a usable X server for anything
built after 1995. Phase 10 is the extension that unlocks the next
three phases (XFIXES, XFCE).

The RENDER pipeline is conceptually simple:

1. **Client-side:** load a TTF font via freetype, rasterize a glyph
   to an 8-bit alpha bitmap, `AddGlyphs` the bitmap to a
   server-side glyph set keyed by a glyph id.
2. **Server-side:** stash the glyph bitmaps.
3. **Client-side:** `CompositeGlyphs8(Over)` with a solid-fill
   source picture (the text color) and a destination picture (the
   window). Each glyph element carries (dx, dy, glyph_id) triples.
4. **Server-side:** for each covered pixel, composite
   `src * mask.alpha` over the destination using Porter-Duff Over.

kxserver's Phase 10 implements exactly enough of RENDER to pump
that hot path: 13 minor opcodes, 5 picture formats, 3 composite
operators. Everything else returns `BadImplementation`.

## What Phase 10 shipped

**Picture formats (5):**
- `PICTFORMAT_A8R8G8B8` (32-bit, alpha in bits 24..32) — the main
  format for drawable-backed pictures.
- `PICTFORMAT_X8R8G8B8` (24-bit, no alpha) — our fb's native layout.
- `PICTFORMAT_R5G6B5` (16-bit) — advertised for completeness.
- `PICTFORMAT_A8` (8-bit alpha-only) — Xft's grayscale glyph format.
- `PICTFORMAT_A1` (1-bit) — bitmap-glyph mode.

**Resources (2 new variants):**
- `Resource::Picture(Picture)` — wraps either a drawable (window/
  pixmap) or a solid ARGB color. Picture format + repeat mode +
  clip rects.
- `Resource::GlyphSet(GlyphSet)` — a `BTreeMap<u32, Glyph>` of
  uploaded glyph bitmaps plus the format they use.

**Wire handlers (13 minor opcodes):**

| Minor | Request | Status |
|---|---|---|
| 0 | QueryVersion | Returns `0.11` (what Xft's probe expects as a floor) |
| 1 | QueryPictFormats | Full reply: 5 formats, 1 screen, 1 depth, 1 visual, 1 subpixel entry |
| 4 | CreatePicture | Drawable-backed picture |
| 5 | ChangePicture | Stub |
| 6 | SetPictureClipRectangles | Parses + stores |
| 7 | FreePicture | |
| 8 | Composite | Src/Over + Clear |
| 17 | CreateGlyphSet | |
| 19 | FreeGlyphSet | |
| 20 | AddGlyphs | A8, A1 (unpacked to A8), A8R8G8B8 |
| 22 | FreeGlyphs | Stub |
| 23 | **CompositeGlyphs8** | The text hot path |
| 26 | FillRectangles | Src/Over + Clear |
| 33 | CreateSolidFill | |

**The compositor (`render_ext.rs`):**

Two inline per-pixel routines, both exact-integer straight-alpha:

```rust
#[inline]
pub fn over_pixel(src: u32, dst: u32) -> u32 {
    let sa = ((src >> 24) & 0xFF) as u32;
    if sa == 0 { return dst; }
    if sa == 0xFF { return src; }
    // ...per-channel: out = src + dst * (1 - src_alpha)
}

#[inline]
pub fn over_pixel_masked(src: u32, dst: u32, mask_alpha: u8) -> u32 {
    // Scale src channels by mask_alpha/255, then composite Over.
}
```

Six unit tests nail the arithmetic: opaque source, transparent
source, zero mask, full mask, half mask (checking the red channel
lands within ±1 of 128), and `div255` matching exact integer divide
for all `v ∈ 0..=65025`. I started with a fast-multiply-shift
approximation and the `div255` test caught that it returns 1 for
`v=128`, so I swapped to `v / 255` which the compiler already
optimizes to a multiply-shift.

## The five-format advertisement

`QueryPictFormats` is another X11 reply with a mildly hostile
layout:

```
32-byte fixed header
+ num_formats × 28-byte PICTFORMINFO
+ per-screen: num_depths + fallback_format
+ per-depth: LISTofPICTVISUAL
+ LISTofCARD32 subpixel orderings
+ pad4
```

I patch `extra_words` post-hoc — same pattern as QueryFont in
Phase 6. Trying to pre-compute the length across three nested
variable-length lists is a recipe for off-by-one bugs.

The smoke test reads back `num_formats=5` and verifies that both
`PICTFORMAT_A8` (0x13) and `PICTFORMAT_A8R8G8B8` (0x10) appear in
the list by walking the 28-byte PICTFORMINFO records at offsets
`32, 60, 88, 116, 144`.

## The CompositeGlyphs8 hot path

This is the request every Xft client sends, probably hundreds of
times per frame. The layout is:

```
byte 1:   op
dword 4:  src picture
dword 8:  dst picture
dword 12: mask_format (0 if none)
dword 16: glyph set id
word 20:  glyph_x   word 22: glyph_y
bytes 24+: LISTofGLYPHELT8
```

Each GLYPHELT8 is a variable-length mini-record: `{nglyphs, 3
pad, delta_x, delta_y, glyph_ids[nglyphs]}` padded to 4 bytes.
The (delta_x, delta_y) move the glyph origin before drawing the
next run; each glyph's own `x_off` advances the origin after.

The handler:

1. Looks up the source picture. If it's a solid-fill picture, pull
   the stored ARGB color out; otherwise (not yet supported) warn.
2. Looks up the destination picture and its drawable.
3. Snapshots the glyph set into a `Vec<(id, w, h, x, y, x_off, y_off, data)>`
   so the rest of the handler can borrow `state.fb` mutably without
   fighting the borrow checker over the `state.resources` borrow
   that would otherwise hold the glyph data.
4. Walks the GLYPHELT8 list, and for each glyph in each element,
   composites the glyph's A8 alpha over the framebuffer using
   `over_pixel_masked(src_argb, dst_pixel, alpha)`.
5. Advances `glyph_x += glyph.x_off` after each glyph.

The smoke test uploads one 4×4 glyph with an alpha checker pattern:

```
alpha[0..16] = [FF, 00, FF, 00,
                00, FF, 00, FF,
                FF, 00, FF, 00,
                00, FF, 00, FF]
```

— and reads back a 6×6 region centered on the glyph origin. The
expected result is that the "on" alpha cells show the solid source
color (red) and the "off" cells show the preserved background
(blue, from a prior `PolyFillRectangle`):

```
GetImage 6x6 around glyph:
  0000ff 0000ff 0000ff 0000ff 0000ff 0000ff
  0000ff 0000ff 0000ff 0000ff 0000ff 0000ff
  0000ff 0000ff ff0000 0000ff ff0000 0000ff
  0000ff 0000ff 0000ff ff0000 0000ff ff0000
  0000ff 0000ff ff0000 0000ff ff0000 0000ff
  0000ff 0000ff 0000ff ff0000 0000ff ff0000
```

Every cell is exactly right. Glyph origin is at (2, 2) inside
the 6×6 window: `(gx=0, gy=0)` → red, `(gx=1, gy=0)` → blue,
`(gx=2, gy=0)` → red, ... The framebuffer lacks alpha channels so
the handler ORs `0xFF000000` onto the destination pixel before
compositing and masks the alpha off on write-back.

## What Phase 10 does NOT do

- **Premultiplied alpha.** Everything is straight alpha. Fine for
  one-shot glyph composites, slower for long pipelines that stack
  multiple Over operations. No client I've tested cares.
- **Non-trivial operators.** Only Clear, Src, Over land; In, Out,
  Atop, Xor, Add, Saturate all fall through to Over. xterm and Xft
  don't touch them in the text path.
- **Transforms.** No `SetPictureTransform`. Scaling and rotation
  need matrix composition in `SAMPLE()` and re-sampling, which is
  Phase 11+ work.
- **Gradients.** `CreateLinearGradient`, `CreateRadialGradient`,
  `CreateConicalGradient` — all unimplemented. Used by Cairo for
  backgrounds but not by xterm.
- **Trapezoids / triangles / poly edges.** Xrender's geometric
  primitives. Cairo uses them for antialiased vector graphics;
  xterm doesn't.
- **Picture → Picture compositing into solid-fill destinations.**
  Composites into a solid-fill dest are warned and ignored.
- **A8R8G8B8 source reads from drawable-backed pictures.** The
  `Composite` handler uses `read_rect` from Phase 5, which pulls
  pixels from the fb/pixmap as-is. For same-format windows this is
  correct; for crossing between A8 mask pixmaps and RGB windows it
  isn't. xterm doesn't hit this.
- **The mask argument to Composite.** Ignored for now.
- **ChangePicture.** Stub — xterm calls it once to set repeat mode
  on the source fill, and it doesn't care whether we honor it
  (since our compositor is repeat-agnostic anyway).

## Regression runs

- `cargo test render_ext`: **6/6** — compositor math verified
  (div255 exact, over_opaque, over_transparent, mask=0, mask=255,
  mask=128 within ±1).
- Phase 4 through 10 host smoke tests: **all 7 PASS**.
- `make test-threads-smp`: **14/14 PASS**.

Phase 9's test needed one adjustment: it previously asserted
`RENDER should be absent`, which Phase 10 invalidates. Changed to a
format check that round-trips any reply.

## Where this leaves the project

kxserver now implements **77 core X11 opcodes + 2 extensions
(BIG-REQUESTS, RENDER)**. Every load-bearing wire path has a host
smoke test. The compositor has unit tests. Phase 11 — XFIXES
region objects + loose ends (cursor images via RENDER, SHAPE
decline, MIT-SHM decline with a clean error path) — is the last
extension layer before Phase 12 can try to boot xfce4-panel. The
RENDER foundation is the hard part; Phase 11 is mostly enums and
reply boilerplate.

## Next step: Phase 11 — XFIXES

```
QueryVersion
CreateRegion / DestroyRegion
SetRegion / CopyRegion / IntersectRegion
SelectSelectionInput   (needed for clipboard-watching)
SetPictureClipRegion   (ties RENDER into XFIXES)
GetCursorImage         (xterm calls it for cursor changes)
```

Maybe ~12 opcodes, no new compositing math. Should be 2–3 days,
not 2–3 weeks like Phase 10. After that, Phase 12 is "try to boot
xfce4-panel and see what opcodes we get BadImplementation for,
then implement them one at a time" — the deferred diagnostic
payoff of owning the whole display-server layer.
