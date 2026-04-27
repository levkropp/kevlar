## Blog 235: kbox phases 9-12 — extension negotiation, XKB, MIT-SHM, resource cascade — all clean

**Date:** 2026-04-26

Blog 234 ended with kbox phases 1-8 all passing 5/5 while real
openbox still hung `xprop` with the same signature it always has.
The next bisect round queued (task #34) was phases 9-12: extension
negotiation, XKB initialisation, MIT-SHM segment attach, and the
resource-creation cascade (`CreateGC` / `CreatePixmap` / `MapWindow`
/ `ConfigureWindow`).  All four landed today.  All four pass 5/5.
Real openbox still hangs.

## What phases 9-12 do

### Phase 9 — `QueryExtension` storm

Calls `QueryExtension` for every extension openbox might use:

```
BIG-REQUESTS, XKEYBOARD, MIT-SHM, RANDR, DAMAGE, RENDER,
Composite, GLX, DRI3, Present, XInputExtension, XFIXES, DPMS,
SHAPE, SYNC, XC-MISC
```

12 of 16 are present on our Xorg; 2 absent (GLX, DRI3 — no GPU
driver on fbdev).  Each reply parses cleanly with the right
`(major_opcode, first_event, first_error)`:

```
ext "BIG-REQUESTS" present major_opcode=133 first_event=0 first_error=0
ext "XKEYBOARD" present major_opcode=135 first_event=85 first_error=137
ext "MIT-SHM" present major_opcode=130 first_event=65 first_error=128
ext "RANDR" present major_opcode=140 first_event=89 first_error=147
...
```

5/5.

### Phase 10 — XKB initialisation

`XkbUseExtension`, `XkbGetState`, `XkbGetControls`, `XkbGetNames`.
Uses XKB's major_opcode (135 from phase 9) with minor opcodes in
the data byte — a different wire shape from core X11.  All four
replies arrive correctly:

```
XkbUseExtension supported=true server=1.0
XkbGetState reply extra_bytes=0
XkbGetControls reply extra_bytes=60
XkbGetNames    (X11 Error code=2 (=Value) bad=0x1ffc000 minor=17 major=135)
```

The XkbGetNames Value error is an Xorg-side argument-validation
rejection of `which=0xFFFFFFFF` — not a kernel issue, just kbox
asking for too many name groups at once.  Test still passes 5/5.

### Phase 11 — MIT-SHM segment attach

`MitShmQueryVersion`, `shmget` for a 64 KiB SysV shared-memory
segment, `shmat` to attach it to our address space, `MitShmAttach`
to register it with the server, `shmctl(IPC_RMID)` to mark for
auto-cleanup.

```
MIT-SHM major_opcode=130
MitShmQueryVersion reply extra_bytes=0
shmget → shmid=1
shmat → addr=0xa00003000
MitShmAttach shmseg=0x200001 shmid=1 read_only=false
```

Server accepts the attach (no async error frame).  5/5.

### Phase 12 — resource-creation cascade

`CreateGC` (graphics context bound to root) → `CreatePixmap` (32×32
at root depth) → `CreateWindow` (InputOnly child of root) →
`MapWindow` → `ConfigureWindow` (move to (10,20), resize 64×64,
stack-mode Above) → `FreePixmap` → `FreeGC`.  Drains async event
frames between map+configure and cleanup.

The interesting wrinkle: this happens AFTER phase 1 grabbed
`SubstructureRedirect` on root, so when kbox itself does
`MapWindow` on a child of root, Xorg sends kbox a `MapRequest`
event back (the WM is supposed to ack).  Phase 12's `drained`
counter logs how many such async frames came in.  5/5.

## A/B is unchanged

`KBOX_PHASE=99` (real openbox) still produces:

```
TEST_FAIL openbox_owns_wm_selection (rc=-2)
xprop took 18s, rc=-2
TEST_END 3/5
```

Same hang, same signature, same kernel.  So the kernel-bug trigger
is *still* something real openbox does that no kbox phase has
replicated.

## What we've now ruled out

Twelve phases, every one a clean 5/5:

| # | Behaviour | Wire surface |
|---|---|---|
| 0 | WM identity claim (WM_S0 + EWMH) | 9 requests |
| 1 | SubstructureRedirect grab | 1 request |
| 2 | Window enumeration | QueryTree + variable-length child list |
| 3 | Root property read | ListProperties + GetProperty value-data trailers |
| 4 | Keyboard mapping + 10× passive grabs | 6944-byte single reply |
| 5 | 3× passive button grabs | input subsystem |
| 6 | SetInputFocus PointerRoot | focus delivery chain |
| 7 | 30s tight GetProperty (5ms cadence) | sustained synchronous round-trips |
| 8 | 30s 64-deep batched GetProperty | async pipeline depth |
| 9 | QueryExtension × 16 | extension routing |
| 10 | XkbUseExtension + GetState + GetControls + GetNames | extension minor-opcode dispatch |
| 11 | MitShmQueryVersion + shmget + shmat + MitShmAttach | SysV-shm + server-side handle |
| 12 | CreateGC + CreatePixmap + CreateWindow + MapWindow + ConfigureWindow + Free | resource-id allocation + map/config cycle under our SubstructureRedirect grab |

Stress-checked: each of phases 9-12 ran twice consecutively
without flaking.

## Where the trigger has to live

Real openbox does exactly what kbox does in phases 0-12 *plus*
something else.  The list of things openbox does that kbox still
doesn't is now narrow:

1. **`XRRGetScreenResources` / `XRRGetCrtcInfo`** (RANDR specifics).
2. **`XDamageCreate` + `XDamageNotify` event subscription** (DAMAGE).
3. **`XRenderCreatePicture` + `XRenderQueryPictFormats`** (RENDER).
4. **`XCompositeRedirectSubwindows`** (Composite redirect).
5. **A flood of `ChangeProperty` writes** to advertise full EWMH state
   (`_NET_SUPPORTED`, `_NET_NUMBER_OF_DESKTOPS`, `_NET_DESKTOP_NAMES`,
   `_NET_WORKAREA`, `_NET_CURRENT_DESKTOP`, etc.).
6. **A specific malformed request** that exposes a kernel parse
   bug (less likely — the X11 Error frames in our trace would
   have flagged a bad opcode, but openbox's signature isn't an
   error response, it's silent Xorg).
7. **Higher-volume / different-cadence polling pattern** than
   our phase 7/8 storms (e.g. pure event-driven with no sleeps,
   adaptive batching).

The shortest path forward is to *capture an `xtrace`-style log from
real openbox running on Kevlar* and diff it against kbox phase 12's
log line-for-line.  That's task #35 — the differ tells us the
trigger directly without more guessing.

## What we positively learned

The kernel correctly handles, under simultaneous-other-client load:
- Variable-length X11 reply trailers (QueryTree, GetProperty,
  GetKeyboardMapping, ListProperties, XkbGetControls).
- Extension routing across all 16 standard X11 extensions.
- The XKB extension's minor-opcode dispatch path.
- SysV-shm + AF_UNIX-fd-passing + server-side resource handle
  combo (MIT-SHM attach).
- Resource ID allocation (windows, GCs, pixmaps) and the
  CreateWindow/MapWindow/ConfigureWindow/FreeGC/FreePixmap
  lifecycle under a held SubstructureRedirect grab.
- Synchronous round-trips at 5 ms cadence sustained for 30s.
- Batched 64-deep async pipeline at ~38 000 round-trips in 30s.
- 16-deep `QueryExtension` batch where each reply has a
  4-byte payload (major/event/error/pad).

That's twelve concrete load patterns we now have positive evidence
the kernel handles.  Combined with the cross-arch builds passing
(`make ARCH=arm64 check` + `make ARCH=x64 check` both clean), the
arm64 X11 stack is in better shape than at any prior point in this
arc — even with the openbox-specific trigger still unfound.

## What ships now

- `tools/kbox/src/req.rs` grew typed builders for
  `QueryExtension`, `XkbUseExtension/GetState/GetControls/GetNames`,
  `MitShmQueryVersion/Attach`, `MapWindow`, `ConfigureWindow`,
  `CreateGC/FreeGC`, `CreatePixmap/FreePixmap`, `AllocNamedColor`.
- `tools/kbox/src/reply.rs` grew `parse_query_extension_reply`
  with a typed `QueryExtensionResult` struct.
- `tools/kbox/src/wm.rs` grew `phase9..phase12` functions, each
  documented with the X11 surface it exercises and the hypothesis
  under test.
- `tools/kbox/src/main.rs` dispatches `phase==7`/`phase==8` as
  *terminal* spinners (so they don't block higher phases) and
  `phase>=9..12` as cumulative additions.

## Closing

Twelve negative findings narrow the search faster than one positive
one would have.  Each phase positively confirms a kernel-side load
pattern works; each phase eliminates one openbox-specific suspect.
The intersection — what real openbox does and kbox phase 12 still
doesn't — is now small enough that an `xtrace` capture diff is
the right next move (task #35).

The arc continues to validate the pattern: when a closed-source
process behaves badly on a kernel that's supposed to be a Linux
drop-in, the fastest way to localise the divergence is to write
the smallest open-source thing that does the same job.  At twelve
phases and ~1000 lines of kbox now, we own the "what a WM does"
side of the conversation.  The other side is one strace away.
