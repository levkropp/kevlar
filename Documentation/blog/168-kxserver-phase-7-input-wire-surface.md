# Blog 168: kxserver Phase 7 — Input Wire Surface, Keymap, Deferred Device Integration

**Date:** 2026-04-13

## What Phase 7 shipped

Phase 7 is the input phase. The plan distinguishes two halves:

1. **Wire-protocol surface.** Twenty-some core requests that every
   X client issues during startup — `GetInputFocus`,
   `GetKeyboardMapping`, `GetModifierMapping`, `QueryPointer`,
   `WarpPointer`, the Grab* cluster, `Bell`, etc.
2. **Actual device integration.** Reading `/dev/input/mice`, finding
   the right keyboard device on Kevlar, routing events to the right
   window.

(1) lands this phase. (2) is a deferred ticket because it needs
device discovery that can only happen when the binary runs inside
Kevlar, which is not the host dev environment.

**New opcodes (21):**
- Grabs: `GrabPointer`(26), `UngrabPointer`(27), `GrabButton`(28),
  `UngrabButton`(29), `GrabKeyboard`(31), `UngrabKeyboard`(32),
  `GrabKey`(33), `UngrabKey`(34), `AllowEvents`(35),
  `GrabServer`(36), `UngrabServer`(37).
- Pointer: `QueryPointer`(38), `WarpPointer`(41).
- Focus: `SetInputFocus`(42), `GetInputFocus`(43).
- Keymap: `QueryKeymap`(44), `GetKeyboardMapping`(101),
  `ChangeKeyboardControl`(102), `GetKeyboardControl`(103),
  `Bell`(104).
- Pointer/modifier mapping: `SetPointerMapping`(116),
  `GetPointerMapping`(117), `SetModifierMapping`(118),
  `GetModifierMapping`(119).

**New modules:**
- `input.rs` — `InputState { pointer_x, pointer_y, mask,
  focus_window, focus_revert_to }`, held on `ServerState`. Pure
  data, no I/O, no globals. The wire handlers read and write this;
  a future `/dev/input/*` reader will update it too.
- `keymap.rs` — hardcoded US QWERTY table mapping X11 keycodes
  (evdev scancode + 8) to (unshifted, shifted) keysym pairs.
  Covers letters, digits, punctuation, arrow keys, modifiers,
  navigation keys. Also publishes a `MOD_MAPPING` array for
  `GetModifierMapping` (Shift_L/R, Control_L/R, Alt_L/R, CapsLock,
  Super_L).

**Extended:**
- `event.rs` grew a full set of "device event" builders —
  `key_press`, `key_release`, `button_press`, `button_release`,
  `motion_notify`, `enter_notify`, `leave_notify`, `focus_in`,
  `focus_out`. These all produce 32-byte wire blocks that the
  eventual device reader will enqueue via `Client::queue_event`.

## The diagnostic deferral

Per the Phase 7 plan, the first thing to do is open every candidate
keyboard device on Kevlar and log what comes out —
`/dev/input/event0`, `/dev/tty`, `/dev/console`, `/dev/tty0` —
because `/dev/input/event0` currently points at `MiceFile` in
`kernel/fs/devfs/mice.rs` and nobody has written down where Xorg
actually gets keystrokes from on this kernel.

That investigation can only happen when the kxserver binary is
running inside Kevlar on real-or-simulated hardware. On the dev host,
`/dev/fb0` returns `EACCES` (we already use a shadow framebuffer) and
`/dev/input/*` is equally locked down. So Phase 7 builds the wire
surface on the host, smoke-tests it there, and files
`tools/kxserver/INPUT_TODO.md` describing exactly what the Kevlar
follow-up needs to do:

- Run the 5-second probe script on each candidate device.
- Record which path yields keystrokes.
- If none: file a "kernel evdev keyboard device" ticket.
- Write the mouse reader against `/dev/input/mice` (~60 lines).
- Write the keyboard reader against whatever wins the probe.
- Wire both into `server::poll_once` so the poll loop watches them
  alongside the X11 sockets.

Splitting like this is a deliberate choice: the wire-protocol surface
is something we can regression-test on CI, without hardware, on every
PR; the device-I/O side is a separate gate that lives on Kevlar.

## The smoke test

`tools/kxserver/tests/phase7_smoke.py` exercises every new handler:

```
base=0x200000
GetInputFocus: focus=0x20 revert=2
after SetInputFocus: focus=0x200001 revert=1
QueryPointer: root=(512,384) win=(502,364) same_screen=1
after Warp(root, 100, 150): pointer=(100,150)
after Warp(rel, +5, -10): pointer=(105,140)
GetKeyboardMapping per_keycode=2
  keycode 38: lower=0x61 upper=0x41       # 'a' / 'A'
  keycode 9: 0xff1b                       # Escape
GetModifierMapping per_mod=2
  shift: 50, 62                           # Shift_L, Shift_R
  control: 37, 105                        # Control_L, Control_R
GetPointerMapping n=5
QueryKeymap reply bytes 32..40: 0000000000000000
GetKeyboardControl auto_repeat=1 led=0 bell_pitch=400
GrabPointer status=0
PASS: Phase 7 smoke test
```

Things worth calling out:

- **Default pointer is at screen center** (512, 384). The window
  is at (10, 20), so win-relative should be (502, 364) — the
  test asserts that exact pair to catch `abs_origin` regressions.
- **Absolute warp** goes to `(100, 150)` when dst = root.
  **Relative warp** (src=0, dst=0) applies (dx, dy) to the current
  position, demonstrated by (+5, -10) from (100, 150) landing at
  (105, 140).
- **Evdev → X11 keycode** shift is (+8), so `KEY_A = 30` becomes
  X11 keycode 38. `GetKeyboardMapping(first=38, count=1)` returns
  `{'a' = 0x61, 'A' = 0x41}`, exactly what xterm expects to see.
- **Modifier mapping** exposes the full Shift/Control keycode
  pairs — Shift_L/R at (50, 62), Control_L/R at (37, 105). The
  test checks them by index into the reply body, so any change to
  `keymap::MOD_MAPPING` layout is caught immediately.

## What Phase 7 does NOT do

- **No real device bytes.** The default pointer is static at screen
  center; `QueryPointer` returns the same thing every call until
  someone explicitly warps the pointer.
- **No event delivery.** The `event.rs` builders exist but nothing
  enqueues them from the server side. A real key/button press
  routed to the right window is Phase 7 follow-up work.
- **Stubbed grabs.** Every `GrabPointer` / `GrabKeyboard` returns
  `GrabSuccess`; the actual "route events exclusively to the
  grabbing client" semantics land in Phase 8 alongside twm.
- **No XKB.** `QueryKeymap` always reports all-zero. Good enough
  for xterm's core-path probing; anything that tries to use XKB
  gets the "unknown extension" reply from Phase 1.
- **`ChangeKeyboardControl` / `Bell` / `SetPointerMapping` / `SetModifierMapping`
  are no-op stubs** that still reply with Success. Clients can
  call them without error.

## Regression runs

- `cargo build --release` (non-PIE static musl): clean.
- Phase 7 host smoke test: **PASS** on first run.
- `make test-threads-smp`: **14/14 PASS**.
- Contract tests unchanged — no kernel modifications this phase.

## What Phase 8 will need

twm. Which means:
- Real event routing (walk the tree, check per-client masks,
  honor grabs).
- `SubstructureRedirect` interception on MapWindow/ConfigureWindow/
  CirculateWindow: the requests are *redirected* to the owning
  client as `MapRequest` events instead of being applied.
- `ReparentWindow(7)` — detach from current parent, reattach,
  emit `ReparentNotify`.
- `SendEvent(25)` with the sent-bit set.
- `SetSelectionOwner` / `GetSelectionOwner` / `ConvertSelection`
  for the clipboard dance.
- `CirculateWindow(13)` for stacking.

At that point the deferred device-integration work also becomes
blocking — twm without real keyboard/mouse is a decoration.
