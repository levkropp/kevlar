# 246 — Openbox works on Kevlar+Alpine arm64; onward to LXDE

`make ARCH=arm64 test-openbox` is **5/5 with ShadowFB on**.  Real
openbox + real Xorg + Alpine arm64 + Kevlar.  No workarounds in
the xorg.conf, no toggles in the test harness — the same Alpine
image that works on Linux works on Kevlar.

That closes a saga that started in blog 233 with kbox phase 0
("can a 50-line Rust program satisfy the openbox WM contract?")
and ended in blog 245 with two MAIR attribute slot bits ("the
arm64 page-table descriptor for /dev/fb0 mmap was wrong").
Thirteen blogs.  One bug.

## The shape of the bug, in one sentence

Kevlar mapped userspace `/dev/fb0` mmap pages with arm64
**Device-nGnRnE** (strict device memory) — a memory type whose
LDP/STP behavior is implementation-defined per the ARM ARM —
and musl's aarch64 `memcpy` uses LDP/STP for the bulk path, so
Xorg's `libshadow` framebuffer-update loop never managed to
fully commit a cursor sprite row, kept marking it dirty, and
hot-spun.

The fix: add MAIR attr2 = 0x44 (Normal Non-Cacheable), route
the fb mmap through it.  That's what Linux does
(`pgprot_writecombine` on arm64).  After this, LDP/STP burst
across the bus cleanly, the row is committed, the damage tracker
moves on, the X server returns to `epoll_pwait`, xprop's reply
arrives, the test passes.

## What the trail produced (besides the fix)

The investigation spawned tooling that's now permanent kit:

- **kbox** (`tools/kbox/`) — a 50-line Rust openbox replacement
  that satisfies the EWMH WM contract.  Used to bisect which
  X11 request was the trigger.  Lives in the disk image at
  `/usr/bin/openbox` (with the apko openbox preserved at
  `openbox.real`).  Selectable by `KBOX_PHASE=N`.
- **kxproxy** (`tools/kxproxy/`) — a Unix-socket proxy that
  captures the openbox↔Xorg byte conversation to a hex log.
  Used to grab the trace once we suspected the bug was somewhere
  in the wire bytes.
- **kxreplay** (`tools/kxreplay/`) — replays a captured C2S byte
  trace verbatim against a real Xorg.  Has chunk-by-chunk
  bisection (`KXREPLAY_INCLUDE`, `KXREPLAY_TAIL_BYTES`,
  `KXREPLAY_TAIL_IDX`).
- **Per-CPU non-stale EL0 PC sampler** (`platform/arm64/interrupt.rs::PER_CPU_IRQ_STATE`)
  — captures user PC / SP / LR / x0..x2 on every IRQ that
  interrupted EL0, with a staleness counter so you can tell the
  CPU is currently running EL0 vs idle.  Reads bytes at LR-4 to
  decode the calling instruction.  Cross-references to
  `/proc/N/maps` (read via `dd if=/proc/N/maps bs=64K` to skip
  procfs's 4 KB read truncation) to identify the library.
  *This is the single most useful debugging primitive the
  project has produced; it converts "process X is hot-looping"
  into "process X is at this exact instruction calling memcpy
  with these exact arguments from this exact library."*
- **Phase 19 reproducer** (`tools/kbox/src/wm.rs::phase19_minimal_cursor_trigger`)
  — three X11 requests
  (`OpenFont` + `CreateGlyphCursor` + `ChangeWindowAttributes(root, CW_CURSOR)`)
  that triggered the hang deterministically.  Stays in tree as
  a regression check: with ShadowFB on, phase 19 must pass.

## A bonus correctness pair

While we were in the mmap path:

- `msync()` no longer skips DeviceMemory VMAs; it now issues
  `dsb sy` (arm64) / `mfence` (x86_64) so userspace's stores hit
  MMIO before the syscall returns.  Closes the "msync is supposed
  to be a flush gate" Linux contract.
- `FBIOPAN_DISPLAY` issues the same barrier.  Single-buffered fb
  has nothing to pan, but the ioctl is also userspace's "commit"
  signal.

Neither was load-bearing for the openbox fix, but both were
divergences from Linux that should be closed.  Now closed.

## Where the desktop stack stands

| Desktop | x86_64 | arm64 | Notes |
|---|---|---|---|
| twm    | ? | ✅ | tested in blog 232 |
| i3     | ✅ | ✅ | apko-built |
| openbox | ✅ | ✅ (this blog) | apko-built; ShadowFB on |
| LXDE   | ✅ | ❌ | apk.static-built; needs apko port |
| XFCE   | ✅ | ❌ | apk.static-built; needs apko port |

The arm64 column lights up as we port build scripts off
`apk.static` (which is a Linux ELF and won't run on the macOS
host) onto `apko` (Go binary, runs natively on macOS arm64).

## Next: LXDE on Kevlar arm64

LXDE = openbox + tint2 (panel) + pcmanfm (file mgr) + feh
(wallpaper) + dbus + fonts.  Most of those are already
apko-resolvable.  The work:

1. Port `tools/build-alpine-lxde.py` from `apk.static` to `apko`,
   modeling after `build-alpine-openbox.py` (which already works
   on arm64).
2. Add a `Section "Device"` block to the lxde xorg.conf with the
   same fbdev driver settings — but **without** the ShadowFB-off
   workaround, since the arm64 fix in blog 245 makes ShadowFB on
   work everywhere now.
3. Run `make ARCH=arm64 test-lxde`, see what additional Xorg /
   GTK / D-Bus requirements surface, fix one at a time.
4. Capture screenshots.

XFCE follows the same playbook with a heavier stack
(xfwm4 + xfdesktop + xfce4-panel + xfce4-session).  Blocked
behind the LXDE port both because LXDE is a smaller test
surface and because LXDE shares openbox as the WM (so any
remaining openbox-side issue manifests there first).

## Status

- arm64 openbox: ✅ shipped, 5/5
- Phase 19 regression: in tree as deterministic libshadow-update
  reproducer
- Per-CPU EL0 sampler: in tree, ready for the next "process X is
  hung" mystery
- Next milestone: arm64 LXDE
