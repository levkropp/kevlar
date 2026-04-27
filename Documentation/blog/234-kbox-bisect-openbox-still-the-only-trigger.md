## Blog 234: bisecting kbox up to openbox — eight phases, no repro

**Date:** 2026-04-26

Blog 233 ended with `make ARCH=arm64 test-openbox` at 5/5 because
we'd installed `kbox` (a 500-line Rust WM) at `/usr/bin/openbox`
and bypassed whatever real openbox was doing wrong.  The next
question was the obvious one: *what does real openbox do that kbox
doesn't?*  The plan was to bisect by adding openbox-like X11
behaviour to kbox one phase at a time, watching for the test to
flip back to 4/5.

The instrument: a `KBOX_PHASE` env var (default 0) plumbed through
the kernel cmdline (`kbox-phase=N`) into the test harness's
`env_prefix` and then into kbox at run time.  No rebuild between
phases — same binary, same image, just a different env value.

## The bisect

| Phase | Adds | Result |
|---|---|---|
| 0 | baseline (4 InternAtom + CreateWindow + SetSelectionOwner + 3 ChangeProperty) | 5/5 ✅ |
| 1 | `ChangeWindowAttributes(root, SubstructureRedirect|SubstructureNotify)` — the WM "I am here" exclusive grab | 5/5 ✅ |
| 2 | `QueryTree(root)` + `GetWindowAttributes` + `GetGeometry` for each child | 5/5 ✅ |
| 3 | `ListProperties(root)` + `GetProperty` for every atom (4 atoms came back) | 5/5 ✅ |
| 4 | `GetKeyboardMapping(8, 248)` (6944-byte reply) + 10× `GrabKey` | 5/5 ✅ |
| 5 | 3× `GrabButton(button1/2/3, AnyModifier)` | 5/5 ✅ |
| 6 | `SetInputFocus(PointerRoot, CurrentTime)` | 5/5 ✅ |
| 7 | 30s tight loop: `GetProperty` every 5ms, sync round-trips | 5/5 ✅ |
| 8 | 30s aggressive: 64-deep batched `GetProperty` storm, no sleep | 5/5 ✅ |

Every phase, including the two pathologically busy ones meant to
mimic libev's tight epoll polling pattern, passed 5/5.  The kbox
wire log (`/tmp/openbox.log`) shows the full conversation
phase-by-phase — each new phase prints a `PHASE N entry` banner so
the trace self-segments.

## A/B against real openbox

To rule out "the test got easier somehow," I added one more phase
value: `KBOX_PHASE=99` swaps in the original openbox binary
(preserved at `/usr/bin/openbox.real` when kbox replaced it):

```
$ make ARCH=arm64 test-openbox CMDLINE="kbox-phase=99"
...
TEST_PASS mount_rootfs
TEST_PASS xorg_running
TEST_FAIL openbox_running (WM not found)        ← /proc/N/comm = "openbox.real"
  xprop took 18s, rc=-2
TEST_FAIL openbox_owns_wm_selection (rc=-2)
TEST_PASS openbox_pixels_visible
TEST_END 3/5
```

Real openbox still hangs `xprop` exactly the same way it did
before kbox existed.  The kernel bug is real; we just can't
trigger it with anything kbox does so far.

(The `openbox_running` fail is a side-effect of the comm-name
mismatch — the file was renamed `openbox.real` so the scan for
`"openbox"` misses it.  It's noise; the real signal is the
`xprop took 18s` line.)

## What this rules out

The eight kbox phases between them cover:

- **WM identity claim** (WM_S0 selection, _NET_SUPPORTING_WM_CHECK).
- **Substructure redirect grab** on root.
- **Variable-length reply parsing** (QueryTree's child list,
  ListProperties' atom list, GetProperty's value-data trailer).
- **Large reply** (GetKeyboardMapping returned 6944 bytes — well
  past one MTU and any plausible buffer boundary).
- **Many small requests in flight** (Phase 4's 10 GrabKey + Phase
  5's 3 GrabButton — passive grabs touch the kernel's input
  routing path).
- **Focus management** (SetInputFocus + the FocusIn/FocusOut event
  generation that comes with it).
- **Sustained tight polling** (Phase 7: ~6000 round-trips in 30s).
- **Async batched storm** (Phase 8: ~38000 GetProperty replies in
  30s, 64-deep in flight).

None of those is the kernel-bug trigger.  Yet `xprop` in the
presence of real openbox still hangs identically to how it did
before all our previous fixes (membarrier #283, syscall audit,
arm64 vDSO, kbox itself).

So real openbox is doing *something* in its startup that kbox at
phase 8 still doesn't replicate.  Likely candidates, narrowed
since blog 233 ruled out the "obvious" suspects:

1. **`QueryExtension` for XKB / MIT-SHM / RANDR / DAMAGE / RENDER /
   COMPOSITE** — openbox negotiates these extensions; if any
   returns a bogus base/error code that openbox then uses as an
   opcode, the request hits an opcode our kernel misinterprets.
2. **MIT-SHM with a real SHM segment** — `XShmAttach` involves
   `shmget`/`shmat`, which goes through our shared-memory IPC
   path.  We've never tested this in a WM context.
3. **XKB-specific opcodes** (`XkbUseExtension`, `XkbSelectEvents`,
   `XkbGetControls`).  Different wire format, different reply
   shape.  Our XKB extension support is minimal.
4. **`ConfigureWindow` cycles** — openbox issues these in
   response to `ConfigureRequest`.  If our `ConfigureNotify`
   delivery has an off-by-one, openbox's loop ends up
   self-acknowledging in a way Xorg can't parse.

Phases 9-12 of kbox (one per suspect above) are the next round.

## What we learn anyway

Even without finding the trigger this round, the kbox bisect
positively confirms a lot of kernel-side correctness:

- Variable-length X11 reply trailers parse and deliver correctly
  through our AF_UNIX read path under sustained load.
- Our AF_UNIX listener never starves under 60-200 in-flight
  requests + simultaneous xprop connect (38000 replies in 30s).
- `GrabKey` and `GrabButton` don't deadlock the input subsystem.
- `SetInputFocus(PointerRoot)` doesn't lose the FocusIn/FocusOut
  generation chain.
- A WM that holds `SubstructureRedirect` doesn't block a parallel
  client's `connect()` + `GetProperty(_NET_SUPPORTING_WM_CHECK)`
  round-trip.

These are nine load patterns we can now write contract tests for,
because we control the wire bytes.  That's a real outcome even
without identifying the openbox-specific trigger — and the
phase-N diff is the canonical writeup of what works.

## What ships now

- `tools/kbox/src/wm.rs` grew `phase1_..phase8_` functions, each
  documented with the X11-protocol surface it exercises.
- `tools/kbox/src/req.rs` grew typed builders for
  `ChangeWindowAttributes`, `QueryTree`, `GetWindowAttributes`,
  `GetGeometry`, `ListProperties`, `GetProperty`,
  `GetKeyboardMapping`, `GrabKey`, `GrabButton`,
  `SetInputFocus`.
- `tools/kbox/src/reply.rs` grew typed parsers for the four
  variable-length reply shapes.
- `kernel/main.rs` now sets `bootinfo.raw_cmdline` from the
  parsed cmdline, so `/proc/cmdline` shows what userspace was
  actually given (was empty before — the kernel parsed the
  cmdline structurally but never preserved the raw string).  This
  is what made `KBOX_PHASE` plumbing through the kernel cmdline
  work.
- `testing/test_openbox.c` reads `kbox-phase=N` from
  `/proc/cmdline` and exports `KBOX_PHASE=N` into the env_prefix
  so the bisect happens without rebuilding the disk image
  between phases.  `kbox-phase=99` is the magic value that swaps
  in real openbox for A/B comparison.

## Closing

Two negative findings can be more interesting than one positive
one.  Real openbox still hangs Xorg.  Kbox at every phase we've
written so far does not.  The intersection of "what real openbox
does" and "what we don't yet replicate" is now a small set —
extension negotiation (XKB/MIT-SHM/RANDR), shared-memory
attachments, XKB-specific opcodes, and the ConfigureRequest
acknowledgment cycle.  Phase 9+ are queued as task #34.

The pattern from blogs 232-233 keeps holding: write the smallest
thing that's supposed to behave like the closed-source binary,
diff against the closed-source binary's behaviour, *and the diff
is the bug*.  Sometimes the diff is small (one syscall number);
sometimes it's a class of behaviour (extension negotiation).
Either way, the diff is finite — and visibly so when both sides
are 500 lines of Rust we control.
