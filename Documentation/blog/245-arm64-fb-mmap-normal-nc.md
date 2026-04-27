# 245 — Real fix: arm64 fb mmap pages must be Normal-NC, not Device-nGnRnE

Blogs 241-244 located the openbox hang in
`libshadow.so::shadowUpdatePacked` — Xorg's framebuffer update
loop hot-spinning on `memcpy(ramfb_mmap+0x17f7fc, shadow+0x...,
40)`.  Blog 244 shipped a workaround: `Option "ShadowFB" "off"`
in the openbox xorg.conf.

That workaround is gone.  This is the real fix:

```
diff --git a/platform/arm64/paging.rs
-    let mut attrs = DESC_VALID | DESC_PAGE | ATTR_IDX_DEVICE | ATTR_SH_ISH
+    let mut attrs = DESC_VALID | DESC_PAGE | ATTR_IDX_NORMAL_NC | ATTR_SH_ISH

diff --git a/platform/arm64/boot.S
-    // MAIR_EL1: attr0 = Device-nGnRnE (0x00), attr1 = Normal WB (0xFF)
-    mov     x0, #0xFF00
+    // MAIR_EL1: attr0 = Device-nGnRnE, attr1 = Normal WB,
+    //           attr2 = Normal Non-Cacheable (0x44).
+    mov     x0, #0xFF00
+    movk    x0, #0x44, lsl #16
```

## Why it works

Kevlar's `map_device_page` was using `ATTR_IDX_DEVICE`
(Device-nGnRnE) for any page mapped from a `mmap_phys_base`-
providing file — currently just `/dev/fb0`.  Device-nGnRnE is
"Strict Order, Non-cacheable, Non-buffering, Non-merging,
no Early-write-acknowledge" — it's appropriate for hardware
registers (PCI BARs, GIC registers) where ordering matters and
re-ordering by the CPU would break the device contract.

But Device memory has a sharp edge: **the architecture's behavior
of multi-register transfer instructions on Device memory is
implementation-defined**.  Per ARM ARM B2.7.2: LDP/STP, LD1/ST1,
and friends to Device memory may either work, raise an alignment
fault, or split into multiple separate accesses.

`musl/aarch64/memcpy` uses LDP/STP for the bulk path — exactly
what Xorg's `libshadow` invokes when copying cursor sprite rows
from the shadow buffer into the framebuffer.  On QEMU's virt
model, the LDP/STP didn't fault, but it also didn't behave like
a clean burst: writes to Device memory partially landed,
something subtle in the CPU/QEMU bus interaction left the
underlying ramfb scan-buffer in a state that made Xorg's damage
tracker keep re-marking the same row dirty.  Tight loop forever.

Linux maps fbdev pages with **Normal Non-Cacheable**
(`pgprot_writecombine`).  That still bypasses the data cache (so
QEMU's external view of ramfb stays coherent) but supports the
full instruction set, including aligned multi-register transfers.
LDP/STP burst across the bus, ramfb sees the full write, the
damage tracker marks the row clean, the X server returns to
`epoll_pwait`.

This patch adds MAIR attr2 = 0x44 (Normal-NC) and routes the
fb mmap path through it.  Phase 19 with ShadowFB **on** now
passes, and so does the default openbox test.

## Two more correctness fixes (blog 244 → blog 245)

While we were here, two related divergences from Linux's mmap
contract got closed:

- **`msync()` no longer skips DeviceMemory VMAs.**  Previously
  `msync(MS_SYNC)` on a /dev/fb0 mmap was a silent no-op.  Now
  it issues `dsb sy` so any pending stores hit MMIO before
  returning.  Linux behaves this way; user code expecting msync
  as a flush gate now gets it.

- **`FBIOPAN_DISPLAY` issues the same barrier.**  We're
  single-buffered so panning is a no-op, but the ioctl also
  serves as userspace's "commit" signal — it's the natural moment
  to flush.

Neither was required to fix the openbox hang (the Normal-NC
change alone does it), but both were divergences from Linux.

## Verification

```
make ARCH=arm64 test-openbox CMDLINE="kbox-phase=19"  → 5/5
make ARCH=arm64 test-openbox                          → 5/5
make ARCH=x64 check                                    → clean
```

The xorg.conf workaround in `tools/build-alpine-openbox.py` is
gone.  `Option "ShadowFB" "off"` is **no longer set**; the
default-on behaviour is what runs in the test now.

## What the trail taught us

This bug took eleven blogs (233-245) to fully nail down.  The
chain:

- 233-238: kbox phase-by-phase (X11 request bisect)
- 239-240: kxproxy/kxreplay (byte-trace bisect)
- 241: 3-request kbox phase 19 reproducer
- 242-243: per-CPU non-stale EL0 PC sampler →
  `libshadow.so::shadowUpdatePacked + 0x214`
- 244: ShadowFB-off workaround
- 245: real fix in arm64 page-table attributes

The recurring pattern: a bug that *looked* userspace ("Xorg
hangs") was a kernel bug all along, hidden behind layers of
Xorg/libshadow/musl indirection.  The non-stale EL0 PC sampler
(commit 2e58c13) is what finally cracked it — turning "hung Xorg"
into "Xorg at this exact instruction calling memcpy with these
exact arguments from this exact library."  That primitive is
worth its weight; it's reusable for any future
"a-userspace-process-is-hot-looping" bug.

The deeper lesson: **arm64 memory attributes are load-bearing.**
Device-nGnRnE is correct for hardware registers; Normal-NC is
correct for framebuffers.  Linux gets this right via
`pgprot_writecombine`; Kevlar didn't until now.  Future
mmap-from-file code paths need to think about which attribute
they need.
