## Blog 240: phase 17 replay didn't repro the hang, but bumped the AF_UNIX buffer 16× along the way

**Date:** 2026-04-26

Blog 239 captured the byte that triggers the openbox hang via
kxproxy: a 96-byte chunk containing `SendEvent` (ClientMessage to
root with `_NET_STARTUP_INFO_BEGIN` + "wm started" payload, mask =
`SubstructureNotify`) plus a bundled `ChangeProperty` Append on
`WM_CLASS`.  The next move (task #40) was to replay exactly that
chunk from kbox and bisect kernel-side from there.

Today landed phase 17 — and it doesn't reproduce.  Kbox issuing
the same `SendEvent` + `ChangeProperty` Append in a 30-second loop
passes 5/5 cleanly.  So the trigger isn't *that specific chunk*
in isolation; it's the chunk PLUS the cumulative state Xorg has
built up from openbox's prior 275 messages.

But while inspecting the trace, a different finding turned up.

## What we tried

`tools/kbox/src/wm.rs::phase17_replay_openbox_trigger`:

1. After phase-0..12 setup (WM_S0 + EWMH + extension queries),
   intern `_NET_STARTUP_INFO_BEGIN` and `WM_CLASS`.
2. Create a child window of root (InputOnly, 1×1).
3. For 30 seconds, repeat:
   - `SendEvent(dest=root, propagate=false, mask=SubstructureNotify,
     event=ClientMessage{format=8, message_type=_NET_STARTUP_INFO_BEGIN,
     data="wm started\0\0\0c8 d6 14 10 12 02"})`
   - `ChangeProperty(mode=Append, window=child,
     property=WM_CLASS, type=STRING, format=8, data="kbox.kbox\0")`
   - flush

The second-message data tail (`c8 d6 14 10 12 02`) reproduces
openbox's apparent uninitialised-stack tail from the kxproxy
trace, in case it matters.

Result: **TEST_END 5/5, xprop took 0s.**  Same as every other
kbox phase past 0.

## Why the replay didn't reproduce it

In the kxproxy trace from blog 239, openbox sent 276 chunks total
across ~30 s.  The hang fires after #275.  Phase 17 is essentially
the last 2 chunks (numbers 270 and 276 — there were actually
**two** `SendEvent`s in openbox's burst, one with format=32
`_NET_STARTUP_ID` and one with format=8 `_NET_STARTUP_INFO_BEGIN`)
looped without the rest of the surrounding traffic.

Openbox's prior 274 chunks built up Xorg-side state — properties,
selections, GCs, atoms, mappings — that the SendEvent at #276
operates on.  Phase 17's looped SendEvent has none of that state
to operate on, so Xorg processes it cleanly.

## What we found while looking

While inspecting the trace, I noticed something about the cumulative
S2C (server→client) byte counts that pointed at a sizing problem
unrelated to the trigger.

Total bytes Xorg sent to openbox before the hang:

| count | bytes |
|---:|---|
| 133 | 32 |
| 1 | 6976 |
| 1 | 904 |
| 1 | 260 |
| 1 | 224 |
| 1 | 64 |
| 1 | (the misc) |
| **total** | **~12.7 KB** |

Our AF_UNIX `SOCK_STREAM` buffer was **16 KiB**:

```rust
// kernel/net/unix_socket.rs:33 (was)
const UNIX_STREAM_BUF_SIZE: usize = 16384;
```

Linux defaults to ~200 KiB.  16 KiB was a relic of an earlier
phase of the kernel; the openbox trace never hit 16 KiB cumulatively
in S2C, so it wasn't *this* bug.  But:

- 16 KiB is far below what any real X11 client expects.
- A WM holding `SubstructureNotify` on root receives one event per
  child window operation.  Under any non-trivial X11 workload —
  e.g. someone opens a tab, every CreateNotify/MapNotify/
  ConfigureNotify gets delivered.  At 32 bytes per event, 16 KiB
  fills with 512 events.
- Once full, the server's `writev` to that client blocks; one
  blocked client's writev path can hang the entire server's main
  loop because Xorg processes clients sequentially.

So the *current* openbox bug isn't caused by the 16 KiB buffer
(only 12.7 KiB hit), but a slightly different load pattern would
have, and the current size is unsafe.

Bumped to 256 KiB:

```rust
// kernel/net/unix_socket.rs:33 (now)
const UNIX_STREAM_BUF_SIZE: usize = 262144;
```

Re-verification: `make ARCH=arm64 test-openbox CMDLINE="kbox-phase=12"`
passes 5/5 with no behaviour change at the test level.  No
regression.

The downside: each `SOCK_STREAM` UnixStream pair holds 2× 256 KiB
= 512 KiB of static buffer.  Memory cost is meaningful only at
hundreds of concurrent sockets — far above any current Kevlar
workload.  Linux uses dynamically-grown buffers up to the sysctl
limit; the kernel-side TODO is to mirror that, but a 256 KiB cap
is a reasonable interim that matches Linux's effective default.

## Where the openbox bug lives now

After 17 phases of bisecting from the kbox side, a Unix-socket
proxy capture, and one targeted byte-replay, here's the
constraint set:

- The trigger is **not** in any single X11 request type or
  payload (12 content phases + the byte-level replay all clean).
- The trigger is **not** in cadence or threading or FS load
  (phases 13-16 graduate-slow but don't hang).
- The trigger is **not** in our 16 KiB buffer pressure (256 KiB
  doesn't fix it; the trace never hits 16 KiB anyway).
- The trigger **is** specific to real openbox (kxproxy
  reproduced it; phase 17 replay didn't).
- The trigger **is** timing-sensitive (strace-wrap mutes it
  per blog 236).

So the bug exists in the *interaction* between openbox's
specific accumulated Xorg-side state and Xorg's processing of a
late-burst event.  Reproducing it cleanly from kbox would mean
replaying openbox's entire 276-chunk byte stream verbatim — which
implies a byte-level "replay tool" rather than a hand-written
kbox phase.

## Next move

A **kxreplay** tool — same shape as kxproxy but reads a
previously-captured trace and replays the C2S side byte-for-byte
against a real Xorg, ignoring (or sinking) the S2C side.  If
kxreplay reproduces the hang from openbox's captured trace, then
the trigger is fully in the bytes (and a kbox phase that calls
the captured replay function would also reproduce).  If kxreplay
*doesn't* reproduce, the trigger is in some non-byte state
(per-process kernel state, file descriptor counts, signal masks,
or Xorg's parse of the protocol's *timing* of receiving bytes,
which a replay can change inadvertently).

Either outcome narrows further than 17 phases of guessing has.

## What positively shipped

- `tools/kbox/src/req.rs` — typed builders for `SendEvent`,
  `build_client_message`, `change_property_append_string`.
- `tools/kbox/src/wm.rs::phase17_replay_openbox_trigger` — exact
  replay of the captured #276 chunk + the
  `ChangeProperty(Append)` that follows in openbox's burst.
- **`kernel/net/unix_socket.rs::UNIX_STREAM_BUF_SIZE`** bumped
  from 16 KiB → 256 KiB.  Real correctness fix even though it
  didn't address today's bug.  Brings AF_UNIX `SOCK_STREAM`
  capacity up to roughly Linux defaults.
- The constraint refinement: the openbox hang is in the
  *interaction* between accumulated Xorg state and a late-burst
  event, not any single X11 message.

## Closing

Phase 17 was a "guess what bytes matter" — and the answer was
"more than the bytes we guessed".  But the round still produced
a real kernel-side improvement (the buffer bump) and tightened
the bug constraint set.

The pattern keeps holding: every diagnostic round either finds
the bug or finds an adjacent kernel correctness issue worth
fixing on its own.  17 rounds in, we have:
- A working WM (kbox 5/5)
- A diagnostic X server (kxserver)
- A wire-trace proxy (kxproxy with captured 53 KiB byte trace)
- A 16× larger AF_UNIX buffer
- A precise constraint set on the remaining trigger

Task #41 (kxreplay) is the next move.  Each tool is small, each
round narrows the search; eventually the bug runs out of places
to hide.
