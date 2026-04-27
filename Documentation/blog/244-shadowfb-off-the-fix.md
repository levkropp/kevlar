# 244 — `Option "ShadowFB" "off"` — the openbox hang fix

`make ARCH=arm64 test-openbox` is **5/5 passing** for the first
time since the test was added.  The fix is one line:

```
# tools/build-alpine-openbox.py
Section "Device"
    Identifier "fbdev"
    Driver "fbdev"
    Option "fbdev" "/dev/fb0"
    Option "ShadowFB" "off"          ← THIS
EndSection
```

## How we got here

The trail across blogs 233–243:

- 233-238: kbox built up phase by phase, never reproduced the hang
- 239-240: `kxproxy` captured the byte trace; `kxreplay` replayed it
- 241: bisected the trigger to **3 X11 requests**
  (OpenFont + CreateGlyphCursor + CW_CURSOR), encoded as kbox phase 19
- 242: kernel-side IRQ-frame sampler turned out to be stale
- 243: replaced with non-stale per-CPU EL0 sampler (with `stale_irqs`
  staleness counter and bytes-at-LR dumper).  Caught Xorg cpu=1 at
  `stale_irqs=0` every IRQ — genuinely running.  Decoded LR =
  `libshadow.so::shadowUpdatePacked + 0x214`, calling
  `bl memcpy@plt` with `dst=ramfb+0x17f7fc, src=shadow+similar,
  n=40 bytes` (one cursor row at 32 bpp).  The same arguments
  every sample = a tight loop.

## Why this fix works

The fbdev Xorg driver, by default, allocates a "shadow" framebuffer
in regular memory.  All drawing happens to the shadow; `libshadow`
runs a damage-tracking loop that copies dirty regions from the
shadow into the real `/dev/fb0` (ramfb on Kevlar) via `memcpy`.

After `CW_CURSOR=cursor_id` is set on root and the cursor sprite
is composited, *something* in Xorg's bookkeeping flips the
cursor's bounding box to "always dirty" on Kevlar specifically.
`shadowUpdatePacked` keeps copying the same 40-byte row from
shadow to fb forever; Xorg never returns to its main `epoll_pwait`
loop; xprop's `GetProperty` sits in the kernel with no reply.

`Option "ShadowFB" "off"` tells the fbdev driver to skip the
shadow entirely — Xorg writes pixels directly to `/dev/fb0`.  No
shadow-update loop, no infinite memcpy, no hang.

The `xorg` test (build-alpine-xorg.py) already had this option;
the `openbox` test config (build-alpine-openbox.py) was missing
it.  That's the only difference — copying it across closes the
loop.

## What this leaves open

The deeper question — *why does `shadowUpdatePacked` loop on
Kevlar but not on Linux?* — is unresolved.  Candidates:

1. ramfb's mmap returns a region that needs an explicit "I just
   wrote" notification to flush; without it, Xorg's damage tracker
   thinks the dst hasn't been updated.
2. The cursor's damage region is being re-added by Xorg's sprite
   code on every iteration, regardless of whether the previous
   copy succeeded — and Linux's fbdev driver short-circuits the
   re-add somehow.
3. Some XDamage extension state on Kevlar diverges, causing the
   region to never be marked clean.

These are Xorg-internal questions; the test passes today, and
phase 19 is preserved as a deterministic reproducer if someone
wants to root-cause the remaining behavior.

## The PC sampler

The non-stale per-CPU EL0 sampler (`PER_CPU_IRQ_STATE` in
`platform/arm64/interrupt.rs`, `last_user_state` /
`last_user_regs` accessors) is reusable — any future "Xorg / a
process is hot-looping" bug can use the same dump format:

```
PID1_STALL: cpu=N stale_irqs=K user_pc=PC sp=SP lr=LR x0=A x1=B x2=C
PID1_STALL: cpu=N insns near lr-8: I1 I2 I3 I4
```

Dumping the bytes at LR-4 (read via `UserVAddr::read_bytes`)
identifies the exact `bl <target>` instruction; the target offset
plus the maps reading via `dd if=/proc/N/maps bs=64K` (avoiding
procfs's 4 KB read truncation) pins down the library and PLT
entry.  In one test run we go from "Xorg hangs" to "Xorg is at
this exact instruction in this exact library calling memcpy with
these exact arguments."

## Status

- `make ARCH=arm64 test-openbox`: ✅ 5/5
- Phase 19 reproducer: ✅ preserved in tree as a known-bad case
  (with `ShadowFB on` it still hangs; with `ShadowFB off` it passes)
- libshadow root cause: ⚠️ open (Xorg-internal damage tracking)
