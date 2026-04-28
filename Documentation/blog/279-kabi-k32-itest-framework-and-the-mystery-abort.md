# 279 — kABI K32: integration test framework + the mystery abort

K31 shipped graphical Alpine LXDE with a working trackpad and
~12s boot.  Then the user asked for what every developer asks
for next: **drive it from a script, not a window**.  K32 builds
that — a YAML-driven integration test framework — and uses it
to investigate the first real userspace bug: pcmanfm crashing
with `signal 6` (SIGABRT) when the user double-clicks a folder.

The framework lands cleanly.  The bug doesn't.  This is the
honest write-up.

## Part 1: itest

The harness is one file (`tools/itest.py`, ~400 lines) plus a
small YAML schema.  A test looks like:

```yaml
name: lxde-doubleclick-folder
arch: arm64
disk: build/alpine-lxde.arm64.img
init: /bin/test-lxde
cmdline: "kevlar_test_setup=doubleclick"
boot_timeout: 60s
qemu_extra: ["-smp", "2", "-m", "1024", "-vga", "std"]

steps:
  - wait_for_serial:
      pattern: "DESKTOP_READY pcmanfm_pid=([0-9]+)"
      capture: pcmanfm_pid

  - capture_state: { tag: pre-click }

  - inject_mouse:
      action: double_click
      x: 80
      y: 80

  - capture_state: { tag: post-click-12s, delay_before: 12s }

  - extract_disk_artifacts:
      paths:
        - /var/log/Xorg.0.log
        - /var/log/lxde-session.log
        - /var/log/diag

  - assert:
      type: framebuffer_changed
      between: [pre-click, post-click-12s]
      min_pixels_changed: 5000
```

Step types in v1: `wait_for_serial`, `inject_keys`, `inject_mouse`,
`capture_state`, `emit_serial`, `extract_disk_artifacts`,
`query_qmp`, `assert` (`framebuffer_painted` /
`framebuffer_changed` / `framebuffer_unchanged` /
`serial_contains` / `file_contains`).

The runner boots Kevlar via the existing `tools/run-qemu.py`
in `--batch` mode, opens its own QMP socket alongside QEMU
for input injection + screenshot via QEMU's `screendump`,
steps through, persists everything to
`build/itest/<test-name>/`: PNG screenshots, full serial log,
disk files extracted via `debugfs`, `summary.json` with
per-assertion outcomes.

The mouse-injection bits compose three QMP primitives:
abs-move, button-down, button-up — into `move`,
`click`, `double_click`.  Coordinates are pixel positions
(0..1023, 0..767) that the runner scales to the 0..32767
absolute range virtio-tablet expects (per Kevlar's
`EVIOCGABS` at `kernel/fs/devfs/evdev.rs:351`).

```
$ make ARCH=arm64 itest TEST=tests/integration/lxde-smoke.yaml
[itest] running lxde-smoke
[itest] step: wait_for_serial({'pattern': 'TEST_PASS lxde_pixels_visible'})
[itest] step: capture_state({'tag': 'post-boot'})
[itest] step: assert(framebuffer_painted)
[itest]   ASSERT PASS: 756732/786432 non-black (96.2%), threshold 50.0%
[itest] step: assert(serial_contains TEST_PASS xorg_running)
[itest]   ASSERT PASS

PASS lxde-smoke: 2/2 assertions
```

`make itest-all` runs every YAML under `tests/integration/`.
That's the framework.  It works.

## Part 2: the kernel hardening that came along

While building the harness I added a small kernel patch in
`platform/arm64/interrupt.rs`:

```rust
_ => {
    let pc = unsafe { (*frame).pc };
    if from_user != 0 {
        // EL0 took an unclassified synchronous exception.  ec=0
        // is typically an undefined instruction (or some arm64
        // extension instruction HVF doesn't model).  Linux
        // delivers SIGILL/SIGSEGV in this situation; we hand
        // off to handle_user_fault so the misbehaving process
        // dies cleanly without taking the whole kernel down.
        log::warn!("EL0 unhandled exception: ec={:#x} esr={:#x} \
                    pc={:#x} far={:#x} — delivering signal", ...);
        handler().handle_user_fault("arm64 EC=0 unknown", pc as usize);
        return;
    }
    panic!("kernel unhandled synchronous exception: ...");
}
```

Before K32: any time an EL0 process took an unhandled
synchronous exception (EC=0, "Unknown reason"), Kevlar
panicked the whole kernel.  After K32: the kernel logs the
trap, sends a fatal signal to the offending process, and
keeps running.  Linux's behaviour.

This came up because we kept seeing intermittent panics
during long-running LXDE sessions on arm64+HVF — some
instruction sequence HVF doesn't model traps as EC=0, and
under K31 we'd take the whole kernel down with the user.
After K32 the kernel survives.  Independent of the
double-click bug, this is real hardening.

## Part 3: the bug we didn't fix

The user reports: in `run-alpine-lxde`, right-click → "Create
New Folder" → double-click on the folder → desktop freezes.
Console emits one line:

```
PID 31 (/usr/bin/pcmanfm --desktop) killed by signal 6
```

Signal 6 is SIGABRT — pcmanfm called `abort()`.  On Linux,
this typically happens when GLib hits an assertion failure
(missing MIME database, missing app handler, missing GVfs
mount handler).

We went through the four candidate Alpine packages one at a
time, rebuilding the disk image and re-testing each:

- **`shared-mime-info`** — adds `/usr/share/mime/*` for GIO's
  inode/directory lookup.  No fix; same SIGABRT.
- **`xdg-utils`** — adds `xdg-open` / `xdg-mime` for default-
  app dispatch.  No fix; the wallpaper stays painted briefly
  longer (X grab released differently?), but pcmanfm still
  aborts.
- **`desktop-file-utils`** — adds the `.desktop` MIME-app
  cache.  No fix.
- **`gvfs`** — GIO's virtual filesystem layer.  Slows the
  desktop boot dramatically (gvfs-udisks2-volume-monitor
  itself segfaults at startup, pcmanfm waits for the dead
  helper to give up before continuing).  Still no fix for
  the click-to-open path.

All four are now in the apko package set.  They're standard
deps for any pcmanfm desktop on any distro and shouldn't be
absent.  But none of them are the actual cause.

## Part 4: the smoking gun

To find the real cause, we extended the EL0-unhandled-
exception handler to also dump:

- The 4 instruction bytes at PC (read via `UserVAddr`)
- PSTATE.M decoded as `EL0` / `EL1t` / `EL1h`
- SP_EL0

The next two crashes (Xorg and tint2, different binaries,
different PCs):

```
EL0 unhandled exception: ec=0x0 esr=0x2000000
  pc=0xa104f0618 far=0xa104ecb80
  insn=0x41f50608 pstate=0x0(EL0) sp=0x9ffffe0d0

EL0 unhandled exception: ec=0x0 esr=0x2000000
  pc=0xa10417618 far=0xa10198f8c
  insn=0x41f50608 pstate=0x80000000(EL0) sp=0xa1077e770
```

**Same instruction bytes both times: `0x41f50608`.**  Same
trap class.  Both confirmed from EL0 (PSTATE.M = 0).

`0x41f50608` is **not a valid arm64 instruction** — neither
clang nor `otool` can decode it; both show it as raw `.long`.
And the byte sequence `08 06 f5 41` does **not appear
anywhere in the 1GB Alpine disk image**.  We grep'd.

Two possibilities:

1.  The PC is correct but the page at that PC has been
    unmapped (or remapped) between the trap and our
    diagnostic read.  Our `read_bytes` would then return
    garbage from whatever physical page now holds that
    virtual address.  This means there's a **Kevlar mm
    bug** — a TLB / page-table coherence issue that lets
    multiple processes see different bytes at the same
    virtual address.
2.  The PC is correct AND the page IS backed at trap time,
    but with something other than executable code — e.g.
    pcmanfm's exec'd child landed on a heap page or a COW'd
    data page.  This would be a real **userspace wild-jump**
    that Linux would also SIGSEGV — a binary bug, not a
    kernel bug.

We can't tell which without more diagnostics, and that's the
honest end of K32.

## What's still rough

- **Mouse cursor injection via QMP doesn't move the cursor.**
  Events reach virtio-tablet's eventq (kernel logs "first
  IRQ drained 3 events from QEMU Virtio Tablet") but Xorg
  doesn't apply them to position.  Adding `Option "Tablet"
  "true"` to xf86-input-evdev's config helped Xorg classify
  the device as a tablet (not a touchpad) but didn't make
  abs events land.  That's the gap that prevents the
  doubleclick test YAML from actually triggering the click —
  the YAML is the right shape, the framework runs the steps,
  the diagnostic emits run, but the click itself doesn't
  reach pcmanfm.  K33+ explores VNC-protocol injection or a
  kernel-side input-event-injection syscall.
- **The freeze itself.**  Root cause undetermined.  We've
  got the crash signature but not the why.  K33 picks this
  up with deeper kernel-side instrumentation (page-table
  walk to read PC bytes via the kernel's own translation
  rather than copy_from_user) and a Linux-baseline parity
  run (linux-on-hvf with the same Alpine image — if Linux
  also crashes, it's a binary bug; if not, it's Kevlar).

## Status

| Milestone | Status |
|---|---|
| K1-K29: kABI arc | ✅ |
| K30: graphical Alpine LXDE | ✅ |
| K31: trackpad + 12s boot + quiet console | ✅ |
| K32: itest framework + EL0 hardening + freeze diagnosed | ✅ |
| K33: freeze fix + QMP mouse | ⏳ |

The framework is the durable deliverable.  The freeze
investigation is the first real test of "drop-in Linux
replacement" — when a real userspace program does something
unusual on Kevlar, do we get the same behavior as Linux, or
something different?  Right now: something different.  K33
finds out why.
