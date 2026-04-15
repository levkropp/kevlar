# Phase 7 follow-up: real device integration

Phase 7 built the full X11 input wire-protocol surface (26, 27, 28, 29,
31, 32, 33, 34, 35, 36, 37, 38, 41, 42, 43, 44, 101, 102, 103, 104,
116, 117, 118, 119) and the US-QWERTY keymap, but it did **not** wire
those to a real input device. The kxserver process never reads from
`/dev/input/*`, `/dev/tty*`, or any evdev-style source.

This works for a smoke test because the Python harness hits every
handler purely through the wire protocol. It does not work for a
running xterm.

## Before writing any driver code: diagnose the keyboard path.

Per the original plan, the keyboard device path on Kevlar is a known
unknown. `/dev/input/event0` is currently wired to `MiceFile`
(`kernel/fs/devfs/mice.rs`) which emits ImPS/2 mouse packets — not
keyboard data. Real Xorg on Kevlar reads keyboard from *somewhere*
that we haven't identified.

### The 30-minute diagnostic

Run kxserver on Kevlar with a tiny probe:

```sh
for dev in /dev/input/event0 /dev/tty /dev/console /dev/tty0; do
    echo "--- $dev ---"
    head -c 64 "$dev" | od -An -tx1 &
    TAIL=$!
    sleep 5
    kill $TAIL 2>/dev/null
done
```

Record which candidate actually yields keystroke bytes. If none do,
file a separate ticket for a kernel evdev keyboard device.

## Mouse path is likely uncontroversial

`/dev/input/mice` already exists on Kevlar and emits 3-byte ImPS/2
packets. A reader in `input.rs` that non-blocking reads the device
fd, decodes byte 0 (buttons) and signed bytes 1/2 (dx, dy), and calls
`InputState::set_pointer(...)` + `InputState::button_{down,up}(...)`
should be ~60 lines of Rust. Don't write it until the keyboard path
is settled — the two need to share a poll loop in `server::poll_once`.

## Event routing

Real events also need delivery routing:
- The event must be sent to *the window under the pointer* (for
  button/motion) or *the focus window* (for key events).
- Walk the parent chain and match each level against the per-client
  event masks (pattern already exists for CreateNotify etc.).
- Grabbed pointer/keyboard state (the stubs in `handle_grab_pointer`
  etc.) must route everything to the grabbing client instead.

## What's already testable

Every wire-level request in the Phase 7 list is covered by
`tests/phase7_smoke.py` on the host. That test is the correctness
gate for the protocol surface; the Kevlar-side device integration
gets its own separate gate once the keyboard path is known.
