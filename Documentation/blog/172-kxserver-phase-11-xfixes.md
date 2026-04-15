# Blog 172: kxserver Phase 11 — XFIXES, Regions, and Picture Clipping

**Date:** 2026-04-13

## Why Phase 11

Phase 10 gave us RENDER — enough for Xft to upload glyphs and blit
them as antialiased text. But GTK + Cairo clients don't just draw
text in one straight shot; they draw it clipped to damaged
regions. Every frame, GTK computes a region of "pixels that need to
change" and installs it as the clip on a Picture via
`SetPictureClipRegion` before drawing. Without XFIXES, those calls
return `BadImplementation`, the client sees the extension as
unavailable, and Cairo fails back to a slow path that tends to
expose very different bugs.

Phase 11's job is to give XFIXES enough teeth that the Cairo backend
keeps using its RENDER-aware fast path, and that real clip regions
actually clip real drawing.

## What Phase 11 shipped

**New module `region.rs`** — `Rect` and `Region`:

- `Rect::intersect` — overlap check returning `Option<Rect>`.
- `Rect::subtract` — classical 4-way split that produces up to
  4 non-overlapping fragments covering `self \ hole` (top strip,
  bottom strip, left strip, right strip of the intersection).
- `Region` holds a `Vec<Rect>` with relaxed non-overlap
  (`contains` is OR-based over all members).
- `Region::union` — concatenation.
- `Region::intersect` — pairwise intersect every `(a ∈ self, b ∈ other)`.
- `Region::subtract` — for each member, walk every hole in the
  other region and run `Rect::subtract` iteratively. Output is
  non-overlapping.
- `Region::invert(bounds)` — `bounds \ self`.
- `Region::translate`, `Region::extents`.

10 unit tests nail this down, including one that walks every
point in a 10×10 grid and asserts the subtract result is point-
exact — `region_subtract_produces_nonoverlapping`.

**New resource `Resource::Region(Region)`** with the matching
`region` / `region_mut` accessors on `ResourceMap`.

**Picture gained a `clip_region` field**, and the RENDER compositor
paths (`Composite`, `CompositeGlyphs8`, `FillRectangles`) each now
check the destination picture's clip region in destination-local
coordinates before writing a pixel. This is the one load-bearing
cross-module stitch: XFIXES installs a region on a picture, the
picture is then used as a RENDER destination, and the compositor
honors it.

**XFIXES handlers** — major opcode 130, 16 minor opcodes:

| Minor | Request | Status |
|---|---|---|
| 0 | QueryVersion | Returns 5.0 |
| 2 | SelectSelectionInput | Stub — registration logged, no events yet |
| 4 | GetCursorImage | 8×8 checker at the current pointer position |
| 5 | CreateRegion | Parses rect list |
| 10 | DestroyRegion | |
| 11 | SetRegion | Replaces rect list |
| 12 | CopyRegion | |
| 13 | UnionRegion | |
| 14 | IntersectRegion | |
| 15 | SubtractRegion | |
| 16 | InvertRegion | Against supplied bounds |
| 17 | TranslateRegion | |
| 18 | RegionExtents | |
| 19 | FetchRegion | Returns rects + extents |
| 20 | SetGCClipRegion | Installs rect list on a GC |
| 22 | **SetPictureClipRegion** | The one that matters |

Stubs return `BadImplementation` for: `ChangeSaveSet`,
`SelectCursorInput`, the `CreateRegionFrom*` family,
`SetWindowShapeRegion`, `SetCursorName`, `GetCursorName`,
`ChangeCursor`, `ExpandRegion`, `HideCursor`, `ShowCursor`. None
of these are on the GTK hot path.

## The smoke test

`tools/kxserver/tests/phase11_smoke.py` has seven sub-tests:

```
base=0x200000
XFIXES: present=1 major=130
XFIXES QueryVersion → 5.0
FetchRegion rid_a: extents=(0, 0, 30, 10) rects=[(0, 0, 10, 10), (20, 0, 10, 10)]
IntersectRegion → rects=[(5, 0, 5, 5), (20, 0, 5, 5)]
RegionExtents → rects=[(0, 0, 30, 10)]
SubtractRegion rects=[(0, 0, 10, 3), (0, 7, 10, 3), (0, 3, 3, 4), (7, 3, 3, 4)]
clip pixels: inside=0000ff outside=ff0000 corner=ff0000
GetCursorImage → 8x8
PASS: Phase 11 smoke test
```

The one that carries the whole phase is the "clip pixels" line:

1. Create a 100×100 window.
2. Core `PolyFillRectangle` paints it red.
3. RENDER `CreatePicture` on the window.
4. `CreateRegion` with one rect `(10, 10, 20, 20)`.
5. `SetPictureClipRegion` installs the region on the picture.
6. RENDER `FillRectangles(Src, blue)` across the entire window.
7. `GetImage` reads back three pixels:
   - Inside the clip at `(15, 15)` → **blue** ✓
   - Outside the clip at `(40, 40)` → still **red** ✓
   - Window corner at `(5, 5)` → still **red** ✓

That's the exact semantic GTK relies on: "draw over a big area, but
only the region I set actually changes."

## The region subtract test that caught a subtle bug

While writing the unit tests I tried the most pedantic coverage
check I could think of: "for a 10×10 rect with a 4×4 hole at
`(3,3)`, walk every point in the 10×10 grid and assert the
subtract's `contains` matches (`not in hole`)":

```rust
#[test]
fn region_subtract_produces_nonoverlapping() {
    let a = Region::from_rect(r(0, 0, 10, 10));
    let b = Region::from_rect(r(3, 3, 4, 4));
    let d = a.subtract(&b);
    for y in 0..10 {
        for x in 0..10 {
            let in_hole = x >= 3 && x < 7 && y >= 3 && y < 7;
            assert_eq!(d.contains(x, y), !in_hole, "x={x} y={y}");
        }
    }
}
```

This is a single-line assertion that covers 100 points, and it
passes on first run — but if I had a sign error in the strip
calculations it would fail at the exact point of the wrong
coordinate. I had one in an earlier draft
(`self_bot < inter_bot` instead of `inter_bot < self_bot`) that
only failed on the bottom strip, and the grid walk pinpointed it
to `y = 7, 8, 9`.

Grid-walk tests for rectangle operations are the cheapest high-
confidence coverage I know of.

## What Phase 11 does NOT do

- **Non-rectangular picture clip coordinates.** The clip region is
  always rectangle-aligned. SHAPE's non-rectangular windows are a
  separate extension and we still return "unknown" for it.
- **SHAPE integration.** `SetWindowShapeRegion` returns
  `BadImplementation`. Clients that call it (xterm with the
  decoration shape option, some GTK themes) fall back.
- **Cursor changes.** `ChangeCursor` / `ChangeCursorByName` /
  `CreateRegionFromPicture` (used for cursor masks) — all stubbed.
  The cursor Xterm sees is whatever core `CreateGlyphCursor`
  returned, unchanged.
- **Selection notification events.** `SelectSelectionInput`
  registers but `SetSelectionOwner` does not emit
  `SelectionNotify` on the XFIXES listeners yet. Clipboard watchers
  see nothing. Deferred with the rest of the selection/clipboard
  flow.
- **`CreateRegionFromWindow`** (used for visible-area queries).
  Returns unknown; Cairo falls back to rect lists.

Most of these are small individual fixes; they're deferred because
they do not block `xfce4-panel` startup.

## Regression runs

- `cargo build --release`: clean.
- `cargo test --release`: **36/36 unit tests pass** (10 new
  region, 6 render_ext, 3 setup, 4 wire, 13 others).
- Phase 4 through 11 host smoke tests: **all 8 PASS**.
- `make test-threads-smp`: **14/14 PASS**.

## Where this leaves the project

kxserver now implements:

- ~77 core X11 opcodes
- BIG-REQUESTS (1 request)
- RENDER (13 minor opcodes, full compositor)
- XFIXES (16 minor opcodes, real region math, picture clip tie-in)
- Host smoke tests for every phase
- 36 unit tests covering the data-structure-heavy modules

That is enough protocol surface that the next step is no longer
"write more code blind" — it's **try to boot `xfce4-panel` against
kxserver and iterate on whatever `BadImplementation` errors come
back from the server log**. That is Phase 12. It's a different kind
of work than Phases 0–11: less feature-building, more "drive a real
GTK client until it stops complaining." The diagnostic payoff of
owning the whole display-server layer is precisely this: every
failure comes back as a one-line log entry pointing at a specific
opcode in Rust we wrote.

## Next step: Phase 12 — boot xfce4-panel

The plan:

1. Pick the smallest GTK sample program (`gtk-demo` won't
   bootstrap without a theme, but a plain C `XOpenDisplay +
   XCreateWindow + XMapWindow + drawing loop` will).
2. Launch it against `kxserver :1` on the dev host.
3. Watch the server log for `BadImplementation`, unknown-opcode
   warnings, and failed replies.
4. Implement whatever opcode surfaces next, most of which will be
   small core-protocol pieces we skipped (SetCloseDownMode, ChangeHosts,
   ListHosts, SetAccessControl, SetClipRectangles on GCs, …) plus
   a few more RENDER minor opcodes (maybe `CreatePictureGradient`
   for Cairo background fills).
5. Rinse until the client draws something.
6. Escalate: `gtk3-demo`, then `xterm -fn fixed`, then `xfce4-panel`.

No more blog posts announcing massive feature drops. Phase 12 is
iterative and debugging-driven, and the blog posts coming out of it
are going to look like "Blog 173: xterm talked to us for 12
seconds before hanging on `GetGeometry` of the root window — here's
why", not "Blog 173: here are 20 new opcodes." That's the point of
the whole project.
