# Blog 169: kxserver Phase 8 — SubstructureRedirect, Reparenting, SendEvent, and Cross-Client Routing

**Date:** 2026-04-13

## What Phase 8 shipped

Phase 8 is the window-manager phase. The goal is for a WM client
(eventually twm) to be able to intercept child-window lifecycle
requests from other clients, reparent those children under decoration
frames, and route synthetic events. Running twm end-to-end on Kevlar
is a separate gate — what lands here is the wire-protocol surface and
the core routing machinery, tested on the host with a fake WM + app
Python harness.

**New opcodes (6):**
- `ReparentWindow` (7)
- `CirculateWindow` (13)
- `SetSelectionOwner` (22), `GetSelectionOwner` (23),
  `ConvertSelection` (24) (stub)
- `SendEvent` (25)

**Interception injected into existing opcodes:**
- `MapWindow` (8): if parent has a `SubstructureRedirect` owner other
  than the caller and the window isn't `override_redirect`, synthesize
  `MapRequest` to the owner and DO NOT apply the state change.
- `ConfigureWindow` (12): same pattern, but synthesize a
  `ConfigureRequest` carrying the full value mask + values.
- `CirculateWindow` (13): same pattern with `CirculateRequest`.

**Cross-client event delivery** — the biggest behavioral change.

## The cross-client routing fix

Before Phase 8 the `queue_event_to_client` helper looked like this:

```rust
fn queue_event_to_client(c: &mut Client, target: u32, ev: [u8; 32]) {
    if target == c.id {
        c.queue_event(ev);
    } else {
        log::warn(format_args!("cross-client event drop: ..."));
    }
}
```

i.e. "if the target is the current dispatching client, push onto its
queue; otherwise drop". That was the Phase 3 limitation called out in
the comment. Fine for single-client tests; completely broken for WM
scenarios where client A selects `SubstructureNotify` on root and
client B creates a child window — the event belongs to A but dispatch
is running against B's request.

The fix is a post-dispatch pending-event queue on `ServerState`:

```rust
pub struct PendingEvent {
    pub target_client: u32,
    pub ev: [u8; 32],
}

pub struct ServerState {
    ...
    pub pending_events: Vec<PendingEvent>,
    ...
}
```

`queue_event_to_client` now pushes cross-client events onto
`state.pending_events`. After `server::poll_once`'s client loop
returns, a drain pass routes each `PendingEvent` to the matching
client's `event_queue` and flushes immediately:

```rust
if !self.state.pending_events.is_empty() {
    let pending = core::mem::take(&mut self.state.pending_events);
    for pe in pending {
        if let Some(target) = self.clients.iter_mut()
            .find(|c| c.id == pe.target_client)
        {
            target.queue_event(pe.ev);
            target.flush_events();
        }
    }
}
```

Same-client events still go straight onto `c.queue_event` inside
dispatch — we only stage the cross-client ones. The drain happens
after the borrow of the current client has already been released, so
the borrow checker is happy.

## SubstructureRedirect interception

The invariant twm relies on: if a WM has selected
`SubstructureRedirect` on the parent of a window, any request that
would change the window's state (map, configure, raise) is *redirected*
to the WM as a `MapRequest` / `ConfigureRequest` / `CirculateRequest`
event instead of being applied. The WM gets to decide whether to allow
the operation (by issuing its own `MapWindow` with the WM as caller,
which passes the interception check because `owner == c.id`) or
substitute different geometry.

I track redirect owners in a flat vec on `ServerState`:

```rust
pub struct RedirectOwner { pub parent: u32, pub client: u32 }
pub struct ServerState { ..., redirect_owners: Vec<RedirectOwner> }
```

`apply_window_values` now takes a `caller` id (so the per-client event
mask actually uses the caller's client id, not the window's owner —
which was fine for the single-client test but silently wrong for a WM
that selects events on another client's window). It returns a
`WindowValuesResult` describing what changed:

```rust
struct WindowValuesResult { new_event_mask: Option<u32> }
```

The callers (`CreateWindow`, `ChangeWindowAttributes`) read
`new_event_mask` and call `update_redirect_owner(state, wid, caller,
mask)` to install or clear the redirect entry.

At map time:

```rust
if !override_redirect {
    if let Some(owner) = find_redirect_owner(state, parent) {
        if owner != c.id {
            let ev = event::map_request(parent, wid);
            queue_event_to_client(c, state, owner, ev);
            return true;          // DO NOT apply the map
        }
    }
}
```

`ConfigureWindow` is the same shape, but parses the full xproto
value-mask + values list up front so it can build a faithful
`ConfigureRequest` event body.

## ReparentWindow

`ReparentWindow(7)` detaches a window from its current parent's
child list, updates its `parent` pointer and position, reattaches to
the new parent's child list, and emits `ReparentNotify` to:

- the window itself (`StructureNotify` listeners)
- the old parent (`SubstructureNotify` listeners)
- the new parent (`SubstructureNotify` listeners)

If the window was mapped, X11 spec says the server implicitly unmaps
it first; we clear the `mapped` flag but don't emit a synthetic
`UnmapNotify` — the matching `MapWindow` that the WM issues next
will put things back visually.

## SendEvent and the sent bit

`SendEvent(25)` carries a 32-byte event block verbatim. The server
flips bit 0x80 on byte 0 to indicate "synthesized", then delivers the
event to whichever listeners on the target window match the supplied
mask. When the mask is 0 the event goes to every listener on the
window regardless of their selection — this is the path clients use
for unconditional client-to-client messages.

The destination field can be 0 (`PointerWindow`), 1 (`InputFocus`),
or a concrete window id. For `PointerWindow` we fall back to root
since pointer-window tracking is not implemented yet; for
`InputFocus` we use `state.input.focus_window` from Phase 7.

## Selection ownership

Minimal, local, and flat:

```rust
pub struct SelectionOwner {
    pub selection: u32,   // atom
    pub owner:     u32,   // window id
    pub client:    u32,
    pub time:      u32,
}
```

`SetSelectionOwner` removes any existing entry for the given selection
atom and pushes a new one (or nothing, if `owner == 0`).
`GetSelectionOwner` scans for the atom and returns the stored window
id, or 0 if not found. `ConvertSelection` is logged but not yet
wired to a `SelectionRequest` event on the owner — this is a known
limitation and tracked for a follow-up once a WM actually exercises
the clipboard dance on Kevlar.

## The smoke test

`tools/kxserver/tests/phase8_smoke.py` spawns kxserver, opens **two**
client connections (`wm` and `app`), and exercises every load-bearing
path:

```
wm base=0x200000  app base=0x400000
wm received 2 events
  16 (sent=False) 100001002000000001004000      # CreateNotify
  20 (sent=False) 140001002000000001004000      # MapRequest
app received 0 events
after wm MapWindow:
  wm events: [19]                               # MapNotify (substructure)
  app events: [19]                              # MapNotify (structure)
after Reparent:
  wm events: [21, 21]                           # ReparentNotify × 2 parents
  app events: [21]
  app ReparentNotify: event=0x400001 win=0x400001 parent=0x200001 @(10,25)
PRIMARY atom = 1
GetSelectionOwner PRIMARY → 0x200001
app ClientMessage events: 1
PASS: Phase 8 smoke test
```

Things that test confirms:
- `wm`'s `CreateNotify` on root arrives cross-client (from `app`'s
  CreateWindow call) — pending-events drain path works.
- `MapWindow` interception: wm gets `MapRequest` instead of
  `MapNotify`; app gets *zero* events (not even its own `MapNotify`,
  because the map didn't happen).
- The subsequent wm-initiated `MapWindow` takes the normal path
  (owner == c.id), both clients see `MapNotify`.
- `ReparentWindow` delivers exactly 1 `ReparentNotify` to `app`
  (structure) and 2 to `wm` (substructure on old parent root + new
  parent frame). Fields `{event, window, parent, x, y}` all check.
- `SetSelectionOwner` / `GetSelectionOwner` round-trips across the
  two clients via the shared `state.selection_owners` vec.
- `SendEvent(event_mask=0)` delivers the ClientMessage to `app` with
  the sent-bit (0x80) set and the original `type`/`window` intact.

## What Phase 8 does NOT do

- **`ConvertSelection`**: the handler currently just logs the
  request. Honoring the owner's `SelectionRequest` → reply with
  `SelectionNotify` chain needs a roundtrip through the owner's
  event queue; parked for when a real clipboard flow shows up.
- **Full grab semantics**: the grab handlers from Phase 7 still
  reply `GrabSuccess` without actually routing events exclusively
  to the grabbing client. Grab-aware event routing needs the input
  events to exist first (Phase 7 follow-up).
- **Stacking order honoring**: `CirculateWindow` rotates the
  parent's child list but the renderer does not yet use stacking,
  so there is no visual effect.
- **`GrabServer` exclusivity**: Phase 7 marked these as stubs and
  Phase 8 does not revisit. A real WM-driven modal flow would
  benefit but twm uses it only around root-menu handling.
- **Running twm end-to-end**: that is the Kevlar-runtime gate that
  needs the deferred Phase 7 device integration plus a working
  initramfs + DISPLAY=:1 wrapper. Tracked in `INPUT_TODO.md` and
  the Kevlar test harness work, not in this blog.

## Regression runs

- `cargo build --release` (non-PIE static musl): clean.
- **Phase 4 through 8 host smoke tests: all PASS** (full regression
  suite across phases after the `apply_window_values` refactor,
  confirming the `caller` parameter change did not break existing
  single-client listener paths).
- `make test-threads-smp`: **14/14 PASS**.
- Contract tests unchanged — no kernel modifications this phase.

## Where this leaves us

The kxserver binary now implements roughly 50 core X11 opcodes across
windows, properties, GCs, drawing, pixmaps, fonts, input, selections,
and window-manager support. Every load-bearing wire path has a host
smoke test. The framebuffer renders real text. Two clients can
coexist with one acting as a WM. The things still missing for a real
twm + xterm session are:

1. Device integration on Kevlar (`tools/kxserver/INPUT_TODO.md`) —
   mouse from `/dev/input/mice`, keyboard from TBD after a
   diagnostic run.
2. Grab-aware event routing once device events exist.
3. `ConvertSelection` → owner's `SelectionRequest` flow for clipboard.
4. A Kevlar test harness that runs twm + xterm and either inspects
   the framebuffer for expected decoration pixels or drives xdotool-
   equivalent synthetic events to verify layout.

With those three items plus a Kevlar-runtime test, kxserver becomes
end-to-end functional as the project's diagnostic X server.
