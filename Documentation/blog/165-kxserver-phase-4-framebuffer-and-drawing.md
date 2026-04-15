# Blog 165: kxserver Phase 4 — Framebuffer, Graphics Contexts, and First Pixels

**Date:** 2026-04-13

## What Phase 4 shipped

Phase 4 puts pixels on a screen. The server now owns a framebuffer,
understands graphics contexts, and honors the basic drawing opcodes
that every X client ends up calling inside its expose handler.

**New opcodes (21):**

- GC management: `CreateGC` (55), `ChangeGC` (56), `CopyGC` (57),
  `SetDashes` (58, stub), `SetClipRectangles` (59), `FreeGC` (60)
- Drawing: `ClearArea` (61), `CopyArea` (62), `PolyPoint` (64),
  `PolyLine` (65), `PolySegment` (66), `PolyRectangle` (67),
  `PolyFillRectangle` (70), `PutImage` (72)
- Colormaps: `CreateColormap` (78), `FreeColormap` (79),
  `AllocColor` (84), `AllocNamedColor` (85), `FreeColors` (88, stub),
  `QueryColors` (91), `LookupColor` (92)

**New modules:**

- `fb.rs` — dual-backend framebuffer. On Kevlar it opens `/dev/fb0`,
  runs `FBIOGET_VSCREENINFO` + `FBIOGET_FSCREENINFO`, and mmaps the
  device. On the dev host (where `/dev/fb0` is permission-denied) it
  falls back to a shadow `Vec<u32>` of the advertised screen size. A
  `dump_ppm()` method writes the current buffer as a P6 PPM, which the
  smoke test uses to verify output without needing a display.
- `colormap.rs` — TrueColor helpers: `pack_pixel(r, g, b)` from 16-bit
  X channels to 0x00RRGGBB, `round_channel_16`, and a ~30-entry named
  color table (white/black/red/…/steelblue) including `#RRGGBB` and
  `#RRRRGGGGBBBB` hex parsing.
- `gc.rs` — `Gc` struct and `apply_values(mask, values)` that parses
  the `GC*` bit table from xproto.h (function, plane_mask, foreground,
  background, line_width, …, arc_mode).
- `render.rs` — `ClipRect`, `fill_rect_window`, `rectangle_outline_window`,
  `line_window` (Bresenham), `put_pixel_clipped`, and `copy_area_fb_to_fb`.
  All drawing clips to the target window's absolute bounds ∩ the
  framebuffer bounds, with an overlap-aware walk for CopyArea.

`state.rs` now owns a `Framebuffer` alongside the atoms and the
resource map. `resources.rs` gained `Resource::Gc(Gc)` and
`Resource::Colormap { visual }` with the corresponding accessors.

## The smoke test

Phase 4's success criterion: a hand-rolled client creates a window,
maps it, creates a GC with `foreground=0x00FF0000`, sends one
`PolyFillRectangle`, and when kxserver dumps the framebuffer the
expected pixels are red.

That is exactly what `tools/kxserver/tests/phase4_smoke.py` does:

```
rid_base=0x200000 mask=0x1fffff
--- kxserver log ---
[--]         WRN framebuffer: /dev/fb0 unavailable (...); using shadow 1024x768
--- end log ---
center=(255, 0, 0) outside=(0, 0, 0)
PASS: Phase 4 smoke test
```

The test window is 200×150 at screen (100, 80). The fill is 180×130 at
window (10, 10), so the center of the red area is at screen (200, 155).
Sampling the dumped PPM there returns exactly `(0xFF, 0x00, 0x00)` and
the root background at (50, 50) is `(0x00, 0x00, 0x00)`.

To make the test possible without a real display I added:

- `--ppm-on-exit=PATH` in `config.rs`.
- `SHUTDOWN_FLAG` in `server.rs`: a `static AtomicBool` flipped by a
  `SIGTERM`/`SIGINT` handler installed from `main`. The poll loop
  checks it each iteration and returns `RunError::Interrupted`.
- `main.rs` calls `server.framebuffer_mut().dump_ppm(path)` on exit if
  the flag is set.

## Design notes

**Coordinates.** `abs_origin(wid)` walks `resources.window(wid).parent`
up to the root, summing `(x, y)`. All drawing ops convert
window-relative → screen-absolute via that helper, then clip against
`window_clip(wid)` (the window's own rect in screen space) and the
framebuffer bounds.

**Clipping is rectangle-based only.** No occlusion yet — the last
window to draw wins. This is the documented Phase 4 limitation; the
dispatcher flag for it is the `# What Phase 4 does NOT do` list
below.

**Pixel format.** The TrueColor visual we advertise in `setup.rs` has
masks `0xFF0000/0xFF00/0xFF`. `pack_pixel` assembles exactly that
layout and `AllocColor`'s reply round-trips it via `round_channel_16`,
so xlib sees a consistent channel shape regardless of which Alloc path
it took.

**GC values table.** `apply_values` is a straightforward lookup of the
23 `GC*` bits against a const slice of `(bit, fn)` pairs. Ignored
fields (tile/stipple pixmap ids, subwindow_mode, graphics_exposures)
are still parsed so the value list stays in sync — otherwise a client
that sets `GCLineWidth` alongside `GCTile` would end up writing the
tile id into `line_width` because we skipped a word.

## What Phase 4 does NOT do

- **Occlusion / overlap clipping.** Sibling windows can overdraw each
  other. Fine for a single-client diagnostic; twm will care in Phase 8.
- **Non-`GXcopy` GC functions.** The enum is parsed and stored but
  drawing always writes the raw pixel.
- **`PutImage` with `XYBitmap` / `XYPixmap`.** ZPixmap at depth 24
  only. Everything else is silently dropped with a log line.
- **`PolyArc` / `PolyFillArc`.** Accepted with their opcode slot but
  not drawn; these are Phase-5/6 work at earliest.
- **Text.** No font in the image yet, so the text opcodes remain
  unhandled. That is Phase 6.

## Regression runs

- `cargo build --release` (static musl, non-PIE): clean, no warnings.
- Phase 4 host smoke test: PASS.
- `make test-threads-smp`: 14/14 PASS.
- `make test-contracts`: 157/159 PASS. The two non-PASS tests
  (`signals.sa_restart` TIMEOUT, `time.nanosleep_basic` elapsed=23ms
  vs expected ≥40ms) are timing-slop regressions that predate Phase 4
  and touch no code kxserver modifies — they live entirely in the
  main kernel workspace while `tools/kxserver/` is a separate Cargo
  workspace.

## What Phase 5 will need

Pixmaps, to give `PutImage`, `CopyArea`, and `PolyFillRectangle`
somewhere other than a window to write to. The current `Framebuffer`
already exposes raw `&mut [u32]` through `pixels_mut()`, so Phase 5
mostly reuses `render.rs` with a `Pixmap { pixels: Vec<u32>, stride: usize }`
passed in place of `&mut Framebuffer`.
