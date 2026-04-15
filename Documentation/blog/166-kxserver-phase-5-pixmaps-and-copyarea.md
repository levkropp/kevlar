# Blog 166: kxserver Phase 5 — Pixmaps, CopyArea, and GetImage

**Date:** 2026-04-13

## What Phase 5 shipped

Phase 5 gives kxserver off-screen drawables. Clients can now allocate a
pixmap, draw into it (or upload bytes into it), and blit it onto a
window or another pixmap. The motivating use case is any client that
double-buffers: build a frame in a scratch pixmap, flip the whole
thing to the window in one CopyArea.

**New opcodes (3):**
- `CreatePixmap` (53), `FreePixmap` (54)
- `GetImage` (73) basic

**Extended opcodes (2):**
- `CopyArea` (62) — now handles all four drawable combinations
  (win→win, win→pix, pix→win, pix→pix)
- `PutImage` (72) — now targets windows OR pixmaps

**New module:**
- `pixmap.rs` — a straightforward `Pixmap { width, height, depth, pixels: Vec<u32> }`
  with `get`/`put` accessors. Tightly packed, stride = width. Depth 24
  and 32 accepted; depth 1 (bitmap) is rejected with `BadMatch`.

**Resources:**
- `Resource::Pixmap(Pixmap)` variant with `as_pixmap` / `as_pixmap_mut`
  accessors, and `ResourceMap::pixmap` / `pixmap_mut` helpers.

**Dispatch helper:**
- `is_drawable(state, id)` — returns true for root window, any window,
  or any pixmap. Used by `CreateGC`, `CreatePixmap`'s parent arg,
  `PutImage`, `CopyArea`, and `GetImage`.

## The borrow-checker shape of CopyArea

CopyArea is interesting because src and dst can be the same resource,
and they can be either a framebuffer-backed window or a Vec-backed
pixmap. The simplest thing that handles all four cases with one code
path is to stage pixels through an intermediate `Vec<u32>`:

```rust
// Read src → vec (immutable borrow of state)
let staged = read_rect(state, src_id, sx, sy, w, h);

// Write vec → dst (mutable borrow of state)
write_rect(state, dst_id, dx, dy, w, h, &staged);
```

`read_rect` walks the drawable's bounds and fills a row-major
`Vec<u32>`; out-of-range reads yield 0. `write_rect` is the mirror
image: for a window it goes through `render::put_pixel_clipped` on the
framebuffer surface; for a pixmap it writes directly into
`pixmap.pixels`.

This costs one extra copy per CopyArea, but it sidesteps every
aliasing and borrow-checker problem we would hit trying to hold two
mutable views into `ServerState.resources` (or one into `resources`
and one into `fb`). For a diagnostic server that's the right
tradeoff — the hot path isn't here.

For the window-source case specifically, I added
`Framebuffer::pixels_read(&self)` so `read_rect` can borrow
`&ServerState` and still see the pixel data. `pixels_mut(&mut self)`
would conflict with the `&state.resources` borrow we already hold.

## GetImage layout trap

I hit a self-inflicted bug while writing this: `wire::pad4(n)` returns
`(n + 3) & !3` — the rounded-up length, not the padding remainder. I
initially wrote `total = byte_count + pad4(byte_count)`, which for a
64-byte payload produced `64 + 64 = 128` and told clients
`extra_words = 32`. The Python smoke test dutifully read 128 bytes,
and the pixel offsets I was checking ended up pointing into the
padding tail — which just happened to look "plausibly wrong" at first
glance.

The fix is one line — `let total = pad4(byte_count);` — but the
lesson is worth keeping: every time a reply body has variable-length
trailing data, double-check whether the helper is rounding up or
returning the delta, and round-trip the layout through a test that
actually parses it.

For the index: `pad4(n)` rounds up; `pad4_rem(n)` returns the padding
amount needed to reach the next 4-byte boundary.

## The smoke test

`tools/kxserver/tests/phase5_smoke.py`:

1. Create a 100×100 window at (200, 100), map it.
2. Create a 4×4 depth-24 pixmap.
3. `PutImage` a red/blue checker into the pixmap (ZPixmap, depth 24).
4. `CopyArea` pixmap → window at offset (10, 10).
5. `GetImage` the same 4×4 region back from the window.
6. Check the returned pixels match the expected checker exactly.
7. After SIGTERM, open the dumped PPM and sample cells (0,0), (1,0),
   (1,1) to verify the on-screen pattern.

```
rid_base=0x200000
GetImage returned 16 pixels:
   00ff0000 000000ff 00ff0000 000000ff
   000000ff 00ff0000 000000ff 00ff0000
   00ff0000 000000ff 00ff0000 000000ff
   000000ff 00ff0000 000000ff 00ff0000
cells: (0,0)=(255, 0, 0) (1,0)=(0, 0, 255) (1,1)=(255, 0, 0)
PASS: Phase 5 smoke test
```

That exercises every new wire path in one run: CreatePixmap decoding,
PutImage into a Vec-backed drawable, CopyArea's stage-and-write loop
hitting pix→win, and GetImage reading from a window (the framebuffer
side of `read_rect`).

## What Phase 5 does NOT do

- **Depth-1 bitmap pixmaps.** Needed for cursor masks and for some
  two-tone icon paths. The `depth != 24 && depth != 32` check rejects
  them with `BadMatch`.
- **`CopyPlane` (63).** One-bit-at-a-time copies from a bitmap plane.
  Not yet wired; xterm and twm do not use it for the drawing we care
  about.
- **`XYPixmap` format in PutImage/GetImage.** ZPixmap only.
- **Fill patterns (tile/stipple).** The GC already parses the fields
  but drawing ignores them.
- **Extending the other draw ops to pixmap targets.** `ClearArea`,
  `PolyPoint`, `PolyLine`, `PolySegment`, `PolyRectangle`, and
  `PolyFillRectangle` are still window-only. The `write_rect` helper
  already exists, so extending them is a per-op refactor — punted
  until a client actually asks.

## Regression runs

- `cargo build --release` (non-PIE static musl): clean.
- Phase 5 host smoke test: **PASS**.
- `make test-threads-smp`: **14/14 PASS**.
- `make test-contracts`: **157/159 PASS** — identical to Phase 4,
  same two pre-existing timing failures (`signals.sa_restart`
  TIMEOUT, `time.nanosleep_basic` elapsed=23ms vs ≥40ms). Neither
  touches code that Phase 5 modifies; `tools/kxserver/` is a
  separate Cargo workspace from the main kernel.

## What Phase 6 will need

Phase 6 introduces a built-in bitmap font so the text opcodes
(`ImageText8`, `PolyText8`, and friends) can draw characters. The
glyphs live in a `const GLYPHS: [[u8; 16]; N]` table generated at
build time from a PSF file, and `OpenFont` collapses every requested
XLFD name to that one font. `QueryFont`'s per-character CHARINFO
reply is where we'll need byte-exact correspondence with real Xorg;
the reference capture is the Phase 6 gate.
