# Blog 167: kxserver Phase 6 — Fonts, Text, QueryFont Layout, and a POLLHUP Race

**Date:** 2026-04-13

## What Phase 6 shipped

Phase 6 is the crunch phase per the project plan: the one where
`QueryFont`'s reply layout has to be byte-exact or xterm refuses to
render text. Two new modules, thirteen new opcodes, and a
framebuffer-visible `H`.

**New opcodes (13):**
- Font management: `OpenFont` (45), `CloseFont` (46)
- Font queries: `QueryFont` (47), `QueryTextExtents` (48),
  `ListFonts` (49), `ListFontsWithInfo` (50),
  `SetFontPath` (51) stub, `GetFontPath` (52)
- Text drawing: `PolyText8` (74), `PolyText16` (75),
  `ImageText8` (76), `ImageText16` (77)
- Stubs: `CreateGlyphCursor` (94), `QueryBestSize` (97)

**New modules:**
- `font_data.rs` — a hand-coded 8x16 bitmap font, copied from
  `platform/x64/fbcon.rs` (same project, same license). Covers
  ASCII letters A–Z, a–z, digits 0–9, and the punctuation kxserver
  clients tend to ask for. Every other codepoint renders as a blank
  cell — a known limitation, flagged for follow-up.
- `font.rs` — `EmbeddedFont`/`FontRef` metadata and
  `draw_text_window(fb, abs, clip, x, y, text, fg, bg, opaque)`,
  which rasterizes one byte per glyph using the stock clip and
  framebuffer pipeline from Phase 4.

**Resources:** new `Resource::Font(FontRef)` variant, used so every
`OpenFont` gets a distinct id that clients can `FreeFont` later.

## The load-bearing bugs

### 1. QueryFont reply layout: twin pads

The X11 spec has **two** 4-byte pads in the QueryFont reply — one
after min-bounds CHARINFO and one after max-bounds CHARINFO. I missed
the second. The client dutifully read `extra_words = 781 → 3124
bytes`, but the server only ever wrote 3096 bytes. `recv` blocked
forever waiting for the missing 28 bytes, and the test hung in
`query_font()`.

The fix is to stop hand-computing `extra_words` and instead build the
full reply into a `Vec<u8>`, then patch bytes 4..8 after the fact with
`(out.len() - 32) / 4`:

```rust
put_reply_header(&mut out, seq, 0, 0);   // length placeholder
// ... write entire body, including CHARINFOs ...
let extra_words = ((out.len() - 32) / 4) as u32;
out[4..8].copy_from_slice(&extra_words.to_le_bytes());
```

For any reply where the fixed header and the variable body interleave
across the 32-byte boundary — QueryFont being the worst offender in
core X11 — post-hoc patching is strictly simpler than pre-computing
byte totals and guaranteed to match what you actually wrote.

### 2. POLLHUP drain race

The uglier bug. The Phase 6 smoke test sends eight requests in a
batch, then `s.close()`. Most of the time the server processed all
eight, but occasionally only the first seven made it through —
`ImageText8` never reached the dispatcher even though the client had
already written it to the socket before closing.

Root cause was in `server.rs::poll_once`:

```rust
if pollfd.revents & libc::POLLHUP != 0 || pollfd.revents & libc::POLLERR != 0 {
    log::info(format_args!("C{} hangup", c.id));
    drops.push(i);
    continue;                          // ← drops WITHOUT reading queued data
}
if pollfd.revents & libc::POLLIN != 0 {
    read_from(c); pump(c, state);
}
```

Linux reports `POLLIN | POLLHUP` together when the peer has closed
its write side but the server still has buffered data from before
the close. The old code saw `POLLHUP`, dropped the client, and the
tail of the batch was lost. It looked racy because with a fast
enough pump-loop the queued requests made it through on the poll
that came *before* the HUP arrived.

Fix: always drain `POLLIN` first, even when `POLLHUP` is set, and
only drop on HUP after the reader has returned EOF or the pending
buffer has been fully pumped:

```rust
let mut saw_eof = false;
if pollfd.revents & libc::POLLIN != 0 {
    match read_from(c) {
        Ok(0) => { saw_eof = true; }
        Ok(_) => {}
        Err(..) => { drops.push(i); continue; }
    }
    if let Err(..) = pump(c, state) { drops.push(i); continue; }
}
// ... POLLOUT flush ...
if saw_eof || pollfd.revents & libc::POLLHUP != 0 {
    drops.push(i);
}
```

This is a generic X-server bug pattern and would have eventually
bitten twm too, so the fix is worth keeping regardless of Phase 6.

## The smoke test

`tools/kxserver/tests/phase6_smoke.py`:

1. Handshake, CreateWindow 400×80, MapWindow.
2. OpenFont "fixed".
3. QueryFont → assert `max_width=8`, `ascent=13`, `first=0`,
   `last=255`, `n_chars=256`. Offsets are computed against
   `head + body` so the interleaved boundary does not matter.
4. QueryTextExtents "HI" → assert width = 16 pixels.
5. CreateGC with `GCForeground | GCBackground | GCFont`
   (fg=green, bg=black, font=fid).
6. PolyFillRectangle the whole window green (so the text box is
   visible against a solid background).
7. ImageText8 "HELLO" at window-relative (50, 30).
8. On exit, dump the framebuffer as PPM and scan the expected cell
   row to verify the exact `H`-glyph bit pattern:

```
y=119: .GG..GG.
y=120: .GG..GG.
y=121: .GG..GG.
y=122: .GGGGGG.
y=123: .GG..GG.
y=124: .GG..GG.
y=125: .GG..GG.
y=126: .GG..GG.
y=127: .GG..GG.
```

The crossbar at y=122 is the `0x7E` row; the vertical bars at
columns 1/2 and 5/6 are the `0x66` rows from the embedded font.
Reading from a real framebuffer PPM proves the entire wire + GC +
render + font pipeline is correct end to end.

## What Phase 6 does NOT do

- **Full ASCII font coverage.** The hand-coded table lives in
  `font_data.rs`; every unassigned slot renders as blank. Enough for
  kxserver diagnostics and the smoke test; swap to a PSF-generated
  const for full coverage.
- **Multiple fonts.** Every `OpenFont` maps to the same built-in
  font. The name the client asked for is logged but otherwise
  ignored. Fine for core text — when we eventually want xterm to see
  real XLFD diversity, we add more `FONT_8X16`-style tables.
- **Real `PolyText8`/`PolyText16` delta handling.** The TEXTITEM8
  list is walked, font switches and deltas are *parsed* (so we stay
  in sync with the wire length), but the per-item delta is ignored
  and each run is drawn at cursor-advance width.
- **Bidi draw direction.** `draw-direction` in QueryFont's reply is
  fixed at LtoR.
- **XYBitmap / XYPixmap font formats.** ZPixmap at depth 24 only.
- **`ListFontsWithInfo` per-font replies.** The request currently
  returns a single terminator reply. A future pass will enumerate
  the stock list.

## Regression runs

- `cargo build --release` (non-PIE static musl): clean.
- Phase 6 host smoke test: **PASS** — full `H` glyph renders at
  the expected pixel positions, fill area untouched outside the
  text cells.
- `make test-threads-smp`: **14/14 PASS**.
- Contract tests skipped — Phase 6 touches only the kxserver
  workspace, no kernel changes. Results would be identical to
  Phase 5: 157/159 with the same two pre-existing timing failures
  in `signals.sa_restart` and `time.nanosleep_basic`.

## What Phase 7 will need

Phase 7 is the input events phase: `/dev/input/mice` for mouse,
plus the keyboard path diagnostic the plan flagged as an unknown.
The first 30 minutes of Phase 7 are purely investigative — open
every candidate device on Kevlar and log what comes out — before
any handler code is written. Expect a separate "kernel evdev
keyboard device" ticket to drop out of that.
