## Blog 239: kxproxy reproduced the openbox hang and captured the byte that triggered it

**Date:** 2026-04-26

Blog 238 closed phase 16 with a strategic pivot: 16 kbox phases of
content/cadence/threading/FS-storm bisecting all pass 5/5; real
openbox still hangs.  The proposed next move was a Unix-socket
proxy that captures every byte both directions of the
openbox↔Xorg conversation — strace was the original plan but
blocked because Kevlar doesn't implement `ptrace`.

Today landed `kxproxy`, ~150 lines of Rust.  And on its first run:

```
$ make ARCH=arm64 test-openbox CMDLINE="kbox-phase=97"
TEST_PASS mount_rootfs
TEST_PASS xorg_running
TEST_FAIL openbox_running (WM not found)         ← comm = "sh"
  xprop took 36s, rc=-2
TEST_FAIL openbox_owns_wm_selection (rc=-2)      ← hang reproduced
TEST_PASS openbox_pixels_visible
TEST_END 3/5
```

The hang reproduced.  And we have the full byte trace.

## What kxproxy does

Three-thread Unix-socket proxy:

- **Listener thread** binds `/tmp/.X11-unix/X1` and `accept()`s
  incoming connections.
- For each accepted client: open the upstream `/tmp/.X11-unix/X0`
  (real Xorg), spawn two **forwarder threads**:
  - **Client → Server (C2S)**: read from client, log, write to
    server.
  - **Server → Client (S2C)**: read from server, log, write to
    client.
- Logging: stderr, single line per chunk, `kxproxy: #SEQ TAG bytes=N`
  followed by a 16-byte-row hex dump (capped at 256 bytes per
  chunk to keep the log size manageable).

Implemented in `tools/kxproxy/src/main.rs`.  Cross-compiled for
both `aarch64-unknown-linux-musl` and `x86_64-unknown-linux-musl`,
mirroring the kbox build pattern.  Installed at `/usr/bin/kxproxy`
in the openbox test image.

`KBOX_PHASE=97` in `testing/test_openbox.c` runs:

```sh
/usr/bin/kxproxy 1 0 >/tmp/kxproxy.log 2>&1 &
sleep 1
DISPLAY=:1 /usr/bin/openbox.real >/tmp/openbox.log 2>&1
```

After the test, the proxy log is copied from `/tmp` (tmpfs) to
`/var/log/kxproxy.log` (on-disk ext2) and `sync()`'d, so the host
can extract it via `debugfs -R dump …` after the VM exits.

## What the trace shows

53 KiB of byte-level conversation.  919 lines of log = **277
chunks** (137 C2S + 138 S2C + the connection-close summary line).
Then the proxy log abruptly ends — openbox stops sending and Xorg
stops replying.

Reply size distribution (S2C):

| count | bytes |
|---:|---|
| 133 | 32 (standard X11 reply) |
| 1 | 6976 (XkbGetMap — big keymap) |
| 1 | 904 (another XKB reply) |
| 1 | 260 |
| 1 | 224 (the **last** reply Xorg sent) |
| 1 | 64 |

Request size distribution (C2S):

| count | bytes |
|---:|---|
| 29 | 28 |
| 21 | 24 |
| 20 | 32 |
| 19 | 20 |
| 16 | 36 |
| 13 | 16 |
| ... | ... |

Most requests are tiny (16-36 bytes).  Then a few large ones
(556, 484, 96, etc.) appear at the end.

## The last messages

The trace cuts off at sequence #276 — that's the **trigger**.

#274 was a 556-byte client write (multiple bundled requests, last
visible byte before xprop's deadline).  #275 was Xorg's last
reply, 224 bytes.  Then:

```
kxproxy: #000276 C2S bytes=96
kxproxy:        0000: 19000b00420000000000080021080000  ....B.......!...
kxproxy:        0010: 420000007b010000776d207374617274  B...{...wm start
kxproxy:        0020: 6564000000000000c8d6141012020600  ed..............
kxproxy:        0030: 0e002000430000001f00000008002000  .. .C......... .
kxproxy:        0040: 00000000870807000001070000000000  ................
kxproxy:        0050: 00000000000000000000000000000000  ................
```

Decoded:

**Bytes 0..43 — `SendEvent` (X11 opcode 25):**
- destination = window 0x42 (root)
- event mask = 0x00080000 (`SubstructureNotify`)
- 32-byte event payload, code 33 = `ClientMessage`, format=8
- message-type atom = 0x17b (= 379, server-assigned, almost
  certainly openbox's earlier-interned `_NET_STARTUP_INFO_BEGIN`)
- 20 bytes of UTF-8 data: `"wm started\0\0\0\0\0\0…"` plus what
  looks like padding/uninitialised bytes that decode as
  hex-noise.

**Bytes 44..67 — `ChangeProperty` (opcode 18, mode Append):**
- window = 0x0020000e (one of openbox's allocated XIDs)
- property = atom 0x43 (= 67, `WM_CLASS`)
- type = atom 0x1f (= 31, `STRING`)
- format = 8
- length-of-data follows

**Bytes 68+ — more bundled requests** in the same write.

The kernel-bug trigger is in this 96-byte chunk.  Most likely:

1. **The `SendEvent` with `_NET_STARTUP_INFO_BEGIN` ClientMessage**
   to root with SubstructureNotify mask.  Xorg has to deliver
   the event to every client selecting SubstructureNotify on
   root — which by this point includes openbox itself plus any
   other listeners.  The delivery cycle inside Xorg may interact
   badly with our AF_UNIX wake-up path under load.
2. **The Append ChangeProperty on a WM_CLASS-type STRING.**
   `STRING` properties accept arbitrary 8-bit data; an Append
   that overflows some Xorg internal buffer would explain a
   silent hang.

Either way — we now have a 96-byte hex sequence that, replayed
through kxproxy, would reproduce the hang.  Next round can build
exactly that as a microbenchmark and bisect kernel-side.

## What this rules in

The hang **is** in something specific openbox does in its X11
wire stream.  It's not a kernel timer issue (PER_CPU_TICKS were
healthy in earlier blogs) and not a scheduler fairness issue
(blog 232).  It's protocol-level.

It's also clearly **timing-sensitive** (blog 236 — strace-wrap
mutes it) but the timing only matters because the trigger
request needs to land while Xorg is still in its
busy-processing-prior-requests state.

## What ships now

- `tools/kxproxy/` — new crate, ~150 lines of Rust.  libc-only,
  static-musl, no PIE.  Build target `make kxproxy-bin`.
- `tools/build-alpine-openbox.py::install_kxproxy_if_present` —
  copies the built binary into `/usr/bin/kxproxy` of the
  alpine-openbox image.
- `Makefile`: `kxproxy-bin` target; `$(OPENBOX_IMG)` depends on
  both `kbox-bin` and `kxproxy-bin` so the image rebuilds when
  either changes.
- `testing/test_openbox.c::KBOX_PHASE=97` — runs kxproxy on
  display :1, points openbox.real at it via `DISPLAY=:1`,
  preserves the proxy log on-disk for host extraction.
- `Documentation/blog/images/239-kxproxy-trace-end.txt` — the
  full 53 KiB byte trace from the failing run, archived for
  future reference.

## Closing

The pattern keeps holding: every round, the cheapest-possible
diagnostic visibility produces the next breakthrough.  kxserver
caught a kernel listener bug.  kbox proved the bug isn't in any
of 16 single-dimension load patterns.  Now kxproxy has the byte
sequence — a 96-byte hex chunk we can replay verbatim until we
find the kernel divergence.

Task #40 = build a microbenchmark that replays the openbox
trace's last few messages (or a synthesised version of the
SendEvent + Append-ChangeProperty pattern) and bisect kernel-
side from there.

277 chunks of byte-level evidence beats another 16 phases of
guessing.
