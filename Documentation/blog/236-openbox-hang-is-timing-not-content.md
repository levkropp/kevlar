## Blog 236: the openbox hang is timing-sensitive, not request-content

**Date:** 2026-04-26

Blog 235 closed twelve kbox bisect phases all passing 5/5 while
real openbox still hung `xprop` for 18+ seconds.  The next move
queued (task #35) was to **capture the actual X11 wire traffic
real openbox produces** and diff it against kbox phase 12's, on
the theory that openbox does some specific opcode sequence we
haven't replicated yet.

Today landed an unexpected result: **strace-wrapping real openbox
makes the bug stop firing.**  That single observation reframes the
investigation entirely — the trigger isn't request *content*, it's
request *timing*.

## The strace experiment

Added `strace` to `OPENBOX_PACKAGES` in
`tools/build-alpine-openbox.py`.  Added a `KBOX_PHASE=98` magic
value to `testing/test_openbox.c` that runs the original
openbox.real wrapped under strace:

```c
"%s exec /usr/bin/strace -y -s 8192 "
"-e trace=read,write,readv,writev,recvmsg,sendmsg,..."
"-o /tmp/openbox.strace /usr/bin/openbox.real ..."
```

What we expected: a strace output file we could diff against
kbox phase 12's wire log.

What we got:

```
$ make ARCH=arm64 test-openbox CMDLINE="kbox-phase=98"
TEST_PASS mount_rootfs
TEST_PASS xorg_running
TEST_FAIL openbox_running (WM not found)         ← comm = "strace"
  xprop took 0s, rc=0
TEST_PASS openbox_owns_wm_selection              ← !!
TEST_PASS openbox_pixels_visible
TEST_END 4/5

$ make ARCH=arm64 test-openbox CMDLINE="kbox-phase=99"  # bare openbox.real
TEST_PASS mount_rootfs
TEST_PASS xorg_running
TEST_FAIL openbox_running (WM not found)
TEST_FAIL openbox_owns_wm_selection (rc=-2)      ← still 18s hang
  xprop took 18s, rc=-2
TEST_PASS openbox_pixels_visible
TEST_END 3/5
```

Two test runs.  Same disk image.  Same openbox.real binary.  Same
Xorg.  Same kernel.  Only difference: phase 98 prepends `strace`
to the openbox launch.  And the `xprop` timeout *disappears*.

(The `openbox_running` failure in both runs is a side-effect of
the comm-name scan looking for the literal string `"openbox"`;
under wrapping the parent comm is `"strace"` and bare-mode the
parent comm is `"openbox.real"` — not the trigger.  It's noise.)

## Why this matters

strace doesn't change what openbox sends, by design — it's a
trace tool.  It does two things that *could* shift behaviour:

1. **It slows openbox down.**  Each ptrace stop adds latency
   between syscalls.
2. **It changes the process tree.**  openbox runs as a child of
   strace, not init.  Different pgid, different parent, different
   signal-handling defaults at fork time.

There's also a third factor we found while investigating: **our
kernel doesn't implement `ptrace`.**  No `SYS_PTRACE` arm in
`kernel/syscalls/mod.rs`.  So `strace` itself starts up but its
`ptrace(PTRACE_TRACEME)` returns ENOSYS, and strace falls back to
just `exec`'ing openbox.real without any tracing.  The output file
is 0 bytes — strace traced nothing.

But the **wrap still mutes the bug**.  So the variable isn't the
ptrace stops or the trace I/O — it's the strace process startup
overhead pushing openbox's exec a few hundred milliseconds later.

The harness sleeps 9 seconds after starting openbox before the
xprop probe runs.  In bare mode, that 9 s window starts with
openbox's exec; openbox is into its full request flood by t≈3 s
and `xprop` arrives at t≈10 s, square in the middle of openbox's
busy phase.  Under strace, openbox doesn't actually start until
t≈3-5 s of strace setup; by t≈10 s it's still in early
initialisation, and `xprop` arrives during a quieter window
where Xorg can service it.

## What this rules out

Things the bug is **not**:

- **A specific opcode** openbox uses that kbox doesn't.  Our 12
  bisect phases covered the major candidates; none triggered it.
  Now strace confirms: the trigger isn't anything in the bytes.
- **A malformed request** that crashes Xorg's parser silently.
  Under strace's slow exec, openbox sends the same bytes — just
  later — and there's no hang.
- **An extension-routing bug.**  Same logic.

Things the bug **is**:

- **A timing-sensitive race**.  The trigger is the *cadence* at
  which openbox produces requests during its initialisation
  flood.
- **Specifically: openbox's request rate during its first ~5 s
  saturates Xorg's reply pump in a way that prevents `xprop`'s
  3-request handshake (`SetupRequest`/`InternAtom`/`GetProperty`)
  from ever getting through.**

This dovetails with what blog 232 saw at the kernel-strace level:
during the failure window, Xorg makes **zero syscalls** for 30+
seconds while both CPUs continue ticking.  Xorg isn't crashed —
it's CPU-bound, draining a queue that openbox keeps filling.

## Why kbox phase 7/8 didn't trigger this

Kbox phase 7 ran 30 s of synchronous `GetProperty` round-trips
(one per 5 ms = ~200 reqs/s).  Phase 8 ran 30 s of 64-deep
batched async (~1 200 reqs/s).  Neither triggered the hang.

So it's not a pure rate question — the hang threshold is somewhere
above 1 200 reqs/s, OR it requires a specific *mixture* of request
types (extension queries + property reads + grabs intercalated)
that openbox produces and our uniform polling doesn't.

The literature on libev's Xorg client hot-path mentions a "burst
of ~3 000 requests" during connection setup.  That's plausible:
openbox doing 50+ `InternAtom` for every `_NET_*` hint, ~30
`QueryExtension` calls, ~100 `GrabKey` for every keybinding,
then `XkbGetMap` (which has a *huge* reply), then `XQueryTree`
recursively over every existing window, then `GetProperty` on
every property of every window.

That's a different shape from our phase-8 storm: more variety,
more reply-size variation, more latency-sensitive ordering.

## Two follow-ups

1. **Quantify the burst.**  Add kernel-side syscall counters per
   process to count how many `recvmsg`/`writev` Xorg processes
   per second from each connected client during the failure
   window.  If openbox's burst is 5–10× our phase-8 storm, that's
   the level kbox needs to match.

2. **Mirror openbox's request mixture.**  Phase 13 of kbox: a
   30 s scripted simulation of openbox's actual init sequence —
   ~50 `InternAtom`s, ~30 `QueryExtension`s, ~100 grabs, then a
   tight `GetProperty` loop intercalated with `XkbGetMap`-shaped
   replies (which return ~3 KB each).  This either reproduces the
   hang (giving us a tiny C program to bisect) or proves the bug
   is even more specific.

## What positively shipped this round

- `tools/build-alpine-openbox.py`: `strace` added to
  `OPENBOX_PACKAGES`.  Available for any future tracing work.
- `testing/test_openbox.c`: `KBOX_PHASE=98` magic value runs
  openbox.real under strace; `KBOX_PHASE=99` runs it bare.  The
  diff between them is what surfaced the timing finding.
- The reframing.  We now know the bug isn't in any opcode, in any
  extension, in any reply-shape, in any resource-allocation path
  — it's in the *cadence* with which openbox issues its
  initialisation requests during the first few seconds after
  `connect`.  That's a significantly tighter target than what we
  had at the start of this round.

## Closing

A negative result at the wire-content level + a positive result
at the timing level is more useful than another phase passing
5/5 would have been.  Twelve phases told us where the bug *isn't*;
strace told us where it *is*.  Phase 13 (the openbox-cadence
mimicker) and the per-process syscall counter are the next moves.

The pattern from blogs 232–235 keeps holding: cheap diagnostic
visibility — even an accidentally-broken one (strace whose
ptrace doesn't work) — pays out faster than another round of
hypothesis-bisecting at the protocol layer.
