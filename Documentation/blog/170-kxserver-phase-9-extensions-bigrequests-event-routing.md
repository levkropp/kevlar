# Blog 170: kxserver Phase 9 — Extensions, BIG-REQUESTS, Device Readers, Real Event Routing

**Date:** 2026-04-13

## Why Phase 9 exists

Phase 8 closed out the window-manager protocol surface. That was
enough for a fake WM + app Python harness to verify every load-
bearing cross-client routing path, but not enough to boot twm or
xterm end-to-end — clients were failing before they even got to the
interesting code because:

1. `QueryExtension` returned `BadImplementation` for everything, so
   xcb / Xlib probes hung instead of falling back cleanly.
2. No `BIG-REQUESTS` meant paste-a-screen-of-text into xterm would
   blow up on a request > 256 kB.
3. The device side of Phase 7 was a giant stub — `InputState` existed
   but nothing ever wrote to it. There was no way for a real mouse
   or keyboard to feed the server, and no way for events to route
   to windows.

Phase 9 fixes all three.

## What Phase 9 shipped

**Extension negotiation (opcodes 98, 99, 127)**

- `QueryExtension` returns `present=1, major=128` for `BIG-REQUESTS`
  and `present=0` for literally everything else (RENDER, XKEYBOARD,
  MIT-SHM, XFIXES, DAMAGE, SHAPE, XINERAMA, XInput, …). This
  unblocks every Xlib/xcb client I've ever tested against this shape
  of server.
- `ListExtensions` returns the one-name list.
- `NoOperation` (127) is a legit X11 opcode; we give it a real
  handler that doubles as an injection sync marker (see below).

**BIG-REQUESTS extension (major opcode 128)**

- One minor request: `BigReqEnable`, minor 0. Reply carries a
  `max-request-length` of 262 144 words (1 MB).
- The wire parser in `dispatch_request` now recognizes the
  length-zero encoding: when `RequestHeader::length_words == 0`,
  bytes 4..8 hold a 32-bit extended length (in 4-byte words) and
  the real body starts at byte 8. The dispatcher normalizes the
  layout into the `[hdr..4, body..]` shape every existing handler
  already expects, so BIG-REQUESTS is entirely transparent once
  enabled.

**Device readers (`src/device.rs`, new module)**

Two optional readers, both best-effort:

- `MouseReader` opens `/dev/input/mice` (ImPS/2 3/4-byte packets),
  decodes button + dx/dy deltas with framing recovery, and emits
  `InputEvent::MouseMotion`/`MouseButton`. A per-reader button-diff
  remembers the previous mask and emits Press/Release edges.
- `KeyboardReader` tries `/dev/input/event0..2` in order, reads
  24-byte Linux evdev `input_event` records, and translates Linux
  scancodes to X11 keycodes via the `+8` rule.

Both readers open non-blockingly with `O_RDWR | O_NONBLOCK`, falling
back to `O_RDONLY` if the device rejects read-write. On the host dev
environment both return `EACCES` (expected — `/dev/input/*` is
root-owned) and the reader fields go to `None`. The server runs
cleanly with no input source; the wire handlers keep working off
`InputState`.

**Real event routing (`route_input_event` in `dispatch.rs`)**

The device readers produce `InputEvent` enum values. `route_input_event`
converts them to wire events and stages them on
`state.pending_events`:

- **Pointer motion** walks the tree from root down to the deepest
  mapped window containing the absolute pointer coordinates
  (front-to-back via the children-list order) and delivers
  `MotionNotify` to every listener in that window's ancestor chain
  that selected `POINTER_MOTION`. Propagation honors each window's
  `do_not_propagate` mask.
- **Mouse button** does the same hit-test and emits
  `ButtonPress`/`ButtonRelease`.
- **Key events** deliver to the `state.input.focus_window`
  (falling back to root if focus points at a destroyed window),
  then propagate up. The modifier bit in `state.input.mask` is
  updated first by consulting `keymap::MOD_MAPPING` — Shift, Lock,
  Control, Alt, Super all work.

**`poll_once` integration**

Mouse + keyboard fds get their own `pollfd` entries between the
listener and client slots. Their `POLLIN` is drained inside the
same iteration as the client loop and fed through `route_input_event`
before the cross-client `pending_events` drain, so a keypress at the
start of iteration N lands in the target client's write buffer by
the end of iteration N.

## The synthetic-injection path

Testing device input on the host is blocked by permissions. So
Phase 9 also adds `--inject=SPEC` CLI flags that queue synthetic
`InputEvent`s. Specs:

```
--inject=motion:DX:DY      # relative mouse motion
--inject=button:N:down     # button press/release (N = 1..5)
--inject=button:N:up
--inject=key:KEYCODE:down  # X11 keycode (not Linux scancode)
--inject=key:KEYCODE:up
```

The queue fires exactly once, and the trigger is explicit rather
than timing-based: **when the server processes a `NoOperation(127)`
request**, it sets `state.inject_armed = true`, and the next
poll_once iteration drains the queue into `route_input_event`.

Why `NoOperation`? Tests need to tell the server "I'm done with
setup" in a way that doesn't race TCP buffering. The obvious first
cut was "fire when at least one client has `seq > 0`", but that
fires as soon as the server processes the first batch of requests —
which on Linux is usually `CreateWindow + MapWindow`, sometimes
missing the trailing `SetInputFocus` that arrived in the next TCP
chunk. The smoke test hit exactly this race: key events routed to
`target=0x20` (root) because focus was still root when the injection
fired, and the test only received MotionNotify + ButtonPress, never
KeyPress. `NoOperation` as a one-line sync marker turned out to be
the cleanest fix — it's a real X11 opcode so the client-side code
is just `s.sendall(struct.pack("<BBH", 127, 0, 1))`, no protocol
abuse.

## The smoke test

`tools/kxserver/tests/phase9_smoke.py` has three sub-tests:

**(A) Extension negotiation**

```
BIG-REQUESTS: present=1 major=128
RENDER: present=0
XKEYBOARD: present=0
ListExtensions: n_names=1
  first: b'BIG-REQUESTS'
BigReqEnable max_len=262144 words
```

**(B) BIG-REQUESTS dispatch**

The test sends a `PolyFillRectangle` whose 16-bit length is forced
to zero and whose extended 32-bit length is used instead. The server
must recognize the length-zero encoding, read bytes 4..8 as the real
length, dispatch the request through the existing PolyFillRectangle
handler, and the framebuffer PPM dump must show a green fill at the
expected coordinates. It does.

**(C) Synthetic input routing**

```
received 3 events
  codes: [6, 4, 2]
  Motion: root=(612,434) win=(512,334)
  Button: 1
  Key: keycode=38
```

- `motion:100:50` moves the pointer from screen center `(512, 384)`
  to `(612, 434)`.
- Test window is at `(100, 100, 800, 500)`, so that position maps
  to window-local `(512, 334)`.
- Test selects `POINTER_MOTION | BUTTON_PRESS | KEY_PRESS` and
  `SetInputFocus` on its own window.
- `NoOperation` arms the injection trigger.
- Next poll iteration fires all three synthetic events.
- Client receives MotionNotify with exact coordinates, ButtonPress
  with `detail=1`, KeyPress with `detail=38`.

## What still isn't done

- **Kevlar device integration remains unverified.** `device.rs`
  opens `/dev/input/mice` and `/dev/input/event[012]` in that order,
  but whether either yields sensible bytes on Kevlar is a known
  unknown — the 30-minute diagnostic from `INPUT_TODO.md` has not
  been run. On the host both fail cleanly with `EACCES`, which
  exercises the "no device" code path; next kxserver session on
  Kevlar is where the actual device selection gets validated.
- **Grab-aware routing** still doesn't redirect events to the
  grabbing client. The Grab* handlers return `GrabSuccess` but the
  router ignores the grab table. Hook is a 10-line change at the
  top of `route_input_event` once the data flow is trusted.
- **Enter/Leave notify, FocusIn/FocusOut.** The event builders are
  in `event.rs` but nothing synthesizes them yet — the pointer
  hit-test in `route_motion` knows when the pointer enters a new
  window but doesn't diff against the previous target.
- **RENDER.** Obviously. That's Phase 10.

## Regression runs

- `cargo build --release` (non-PIE static musl): clean.
- Phase 4 through 9 host smoke tests: **all PASS** (6/6).
- `make test-threads-smp`: **14/14 PASS** — no kernel changes.

## What Phase 10 will need

RENDER. Specifically:
- Picture formats negotiation (A8, A1, R5G6B5, A8R8G8B8, …).
- Picture resources backed by windows, pixmaps, or glyph sets.
- Glyph sets with `AddGlyphs` / `FreeGlyphs` and
  `CompositeGlyphs8/16/32`.
- Core `Composite` operation with the Porter-Duff `over` at minimum;
  `src`, `src_in`, `src_atop` are also widely used.
- `FillRectangles` on a Picture (xterm's cursor rendering path).
- `CreateCursor` + RENDER cursor via `Picture`.

That's the investment that unlocks Xft-rendered antialiased text,
which unlocks xterm-with-Xft, gtk-hello-world, and eventually xfce.
Before any of that, though, Phase 9's device integration needs to
earn a green check on Kevlar itself.
