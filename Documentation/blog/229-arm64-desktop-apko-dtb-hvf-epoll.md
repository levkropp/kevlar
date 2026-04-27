## Blog 229: arm64 desktop unblocked — apko, DTB virtio-mmio, HVF-safe MMIO, and an `epoll_event` ABI bug

**Date:** 2026-04-24

Two months of arm64 port work (blogs 207–228) landed fork/exec/spawn
at parity with x64 but never ran Alpine userspace.  Blog 204
documented the x64 XFCE/LXDE bug hunt stalling on `KERNEL_PTR_LEAK`
and the `rip=0 lockdep` panic.  The obvious next experiment — try a
third DE to narrow the bug class — was the motivation for adding
i3.  Instead, the first attempt to boot the i3 stack on arm64
exposed a chain of blockers that had never shipped through a real
workload: the arm64 DTB parser didn't discover virtio devices; HVF
asserted on virtio-mmio probes; and **`struct epoll_event` was laid
out wrong on arm64**, crashing every libev/libuv-based X11 client
the instant `epoll_wait` returned.

This post covers five independent fixes that took the arm64 path
from "doesn't mount a disk" to "Xorg + i3 + xterm running on top of
Alpine aarch64 under HVF."

## Build path: apko on macOS instead of `apk.static`

The existing `build-alpine-xfce.py` / `build-alpine-lxde.py` shell
out to `apk.static` — a Linux x86_64 ELF, which doesn't run on
darwin-arm64.  Every Kevlar desktop test run in blog 150–204 was on
a Linux host.

[apko](https://github.com/chainguard-dev/apko) from Chainguard is a
Go reimplementation of "install a package set into a rootfs" with
no Linux-syscall dependency.  It's in Homebrew:

```sh
brew install apko
```

`tools/build-alpine-i3.py` now uses `apko build-minirootfs` with a
tiny inline YAML config that lists the package set + Alpine 3.21
repositories.  `mke2fs` (from `brew install e2fsprogs`, keg-only)
turns the extracted directory into an ext2 image.  Zero Linux-binary
dependencies on the build host.  `--arch aarch64|x86_64` chooses
which Alpine arch to download .apk files for.

This matters because on Apple Silicon, aarch64 Alpine runs under
QEMU+HVF at native speed.  x86_64 Alpine under HVF would need Rosetta
guest support (slow) or TCG emulation (much slower).  Switching the
desktop test to aarch64 is a 5–10× speedup over what the x64 tests
see on a macOS dev box.

## Fix 1: DTB virtio-mmio discovery on arm64

Symptom at first run:

```
Kevlar ARM64 booting...dtb_paddr = 0x48000000
cmdline: (empty)
...
TEST_FAIL mount_rootfs (errno=19)
```

`ENODEV` on `mount("/dev/vda", ...)`.  The log shows `bochs_fb`,
`virtio_blk`, and `virtio_net` kexts loaded but no `virtio-blk:
driver initialized` line — no probe ran.

`platform/arm64/bootinfo.rs` was parsing the DTB for RAM areas, CPU
MPIDRs, and `/chosen/bootargs`, but virtio device discovery was
commented out with a 2025-vintage note about TCG probe cost:

```rust
// Skip virtio-mmio probing in default mode — each probe is ~1.5s
// under TCG emulation (32 probes = ~48s, exceeds test timeout).
let virtio_mmio_devices = ArrayVec::new();
```

Devices could only be added via kernel cmdline
(`virtio_mmio.device=SIZE@ADDR:IRQ`), which nothing set.  No arm64
test up to now needed a disk.

The fix walks the DTB's top-level `virtio_mmio@<addr>` nodes,
reads each node's `reg = <addr_hi addr_lo size_hi size_lo>` and
`interrupts = <type spi_num flags>` properties, and maps SPI `N`
to GIC INTID `N + 32`:

```rust
if in_virtio_mmio && prop_name == "reg" && prop_len >= 16 {
    let base_hi = *(prop_data as *const u32);
    let base_lo = *(prop_data.add(4) as *const u32);
    vmmio_base = Some(be64_from_cells(base_hi, base_lo) as usize);
}
if in_virtio_mmio && prop_name == "interrupts" && prop_len >= 12 {
    let irq_type = be32(*(prop_data as *const u32));
    let spi_num = be32(*(prop_data.add(4) as *const u32));
    if irq_type == 0 {
        vmmio_irq = Some((spi_num + 32) as u8);
    }
}
```

Result: all 32 virtio-mmio slots QEMU virt exposes are discovered
(2 populated with devices, 30 empty placeholders), virtio_blk finds
device_id=2 at slot 30 and attaches.  `TEST_PASS mount_rootfs` —
**first-ever arm64 Kevlar disk mount.**

## Fix 2: HVF-safe MMIO accessors

After Fix 1 the kernel crashes differently under `--kvm` (HVF on
Apple Silicon):

```
Assertion failed: (isv), function hvf_handle_exception,
file hvf.c, line 2181.
```

`platform/arm64/gic.rs` already documented this exact quirk:

```rust
// Plain `ldr w, [x]` — no post-/pre-index.  HVF's EC_DATAABORT path
// asserts ESR.ISV in qemu target/arm/hvf/hvf.c; post-indexed loads
// clear ISV on Apple Silicon's stage-2 trap, crashing the hypervisor.
core::arch::asm!("ldr {v:w}, [{a}]", v = out(reg) v, a = in(reg) addr,
                 options(nostack, preserves_flags, readonly));
```

The virtio probe paths and the virtio-mmio transport, however, used
plain pointer deref / `ptr::read_volatile`.  LLVM is free to lower
those to instruction forms HVF can't decode — and does, for the 32
back-to-back `ldr w, [x0, #N]` pattern the probe generates.

Added `VAddr::mmio_read{8,16,32,64}` / `mmio_write{8,16,32,64}` in
`platform/address.rs`.  On aarch64 they use inline asm with plain
`ldr/str w/x, [x]`; on x86_64 they fall through to
`ptr::read_volatile` / `ptr::write_volatile`.

Sites switched over:
- `exts/virtio_blk/lib.rs::probe_virtio_mmio` (3 u32 reads)
- `exts/virtio_net/lib.rs::probe_virtio_mmio` (3 u32 reads)
- `libs/virtio/transports/virtio_mmio.rs` — every transport register
  read/write (~20 sites).  The probe fix alone isn't enough: once a
  device attaches and the driver touches `QueueReady`, `DeviceStatus`,
  `FeatureSel`, etc., those had the same latent crash waiting.

HVF run no longer crashes.  Xorg launches in **0.57 s** vs **1.10 s**
under TCG — confirmed native speed.

## Fix 3: RAM-backed framebuffer on arm64

`bochs_fb` only implements `probe_pci` (vendor 0x1234:0x1111).  QEMU
`-machine virt` arm64 has no legacy PCI — PCIe is available via ECAM
but Kevlar's arm64 port doesn't enumerate it yet.  Without PCI
discovery, `bochs_fb` never initializes, `/dev/fb0`'s ioctls return
`ENODEV`, and Xorg's fbdev driver reports *"No devices detected"*.

A full fix (PCIe ECAM scanner + bochs-display over PCIe, or a
virtio-gpu driver) is too big for one session.  The expedient fix:
allocate 3 MiB of kernel RAM at boot and expose it as the fb backing:

```rust
#[cfg(target_arch = "aarch64")]
{
    use kevlar_api::mm::{alloc_pages, AllocPageFlags};
    const FB_W: u32 = 1024;
    const FB_H: u32 = 768;
    const FB_BPP: u32 = 32;
    let num_pages = ((FB_W * FB_H * (FB_BPP / 8)) as usize).div_ceil(4096);
    if let Ok(paddr) = alloc_pages(
        num_pages,
        AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
    ) {
        bochs_fb::init_ram_backed(paddr, FB_W, FB_H, FB_BPP);
    }
}
```

The fb isn't scanned out to any QEMU display device — QEMU arm64 virt
has no pipe to stream from, absent `ramfb`/`virtio-gpu`/PCIe.  But
Xorg/fbdev now opens `/dev/fb0`, reads sensible `fb_var_screeninfo`
and `fb_fix_screeninfo`, and drives it as a normal 1024×768×32 fbdev.
The test harness `mmap`s the same physical region and can dump the
rendered pixels back for offline rendering.  Visible screenshots need
another pass (ramfb via fw_cfg is the natural next step).

## Fix 4: `struct epoll_event` layout is arch-dependent

With fixes 1–3 in, Xorg opens `/dev/fb0` successfully (`EV(0): using
/dev/fb0`), then promptly segfaults:

```
SIGSEGV: pid=4 cmd=/usr/libexec/Xorg fault_addr=0x10596fb0
  ip=0xa001087ec code: 03 08 40 f9 83 00 00 b4 02 0c 40 f9 00 00 40 b9
```

Three independent runs show:
- `pid=4` (first Xorg) always faults at `0x10596fb0`
- `pid=6` (forked Xorg) always faults at `0x1c4b13f0`

**Same IP, different fault addresses, deterministic per PID.**
Low-32-bit values.  Not random stale garbage (which would vary run
to run), not the x64 `KERNEL_PTR_LEAK` shape (which has
`0xffff_8000_XXXXXXXX` kernel direct-map pointers).  This smells
like a truncation.

Extracting the Xorg binary from the guest disk image and
disassembling around the IP shows `WaitForSomething`:

```asm
bl  epoll_wait@plt
mov w20, w0                 ; w20 = number of events
...
ldr w2, [x19]               ; events = u32 at offset 0
ldr x0, [x19, #0x8]         ; data.ptr = u64 at offset 8   ← HERE
...
ldr x3, [x0, #0x10]         ; deref data.ptr → CRASH at fault_addr
...
add x19, x19, #0x10         ; advance by 16 bytes
```

Xorg reads each `struct epoll_event` with a **16-byte stride**, with
`data.ptr` at **offset 8**.  Kevlar's `kernel/syscalls/epoll.rs` wrote
them with a 12-byte stride and `data` at offset 4.  So userspace was
reading 4 bytes of `events` concatenated with 4 bytes of the
*previous* event's `data` tail, casting that as a pointer.

The root cause is one of the oldest Linux ABI warts:

```c
// <sys/epoll.h>
struct epoll_event {
    uint32_t events;
    epoll_data_t data;  // union, 8 bytes
} __EPOLL_PACKED;

#ifdef __x86_64__
#define __EPOLL_PACKED __attribute__((packed))
#else
#define __EPOLL_PACKED
#endif
```

On x86_64 the struct is packed (for ABI compat with 32-bit i386 —
`events(4) | data(8)` = 12 bytes).  On **every other arch** it has
natural alignment (`events(4) | _pad(4) | data(8)` = 16 bytes).

Kevlar's code was `EPOLL_EVENT_SIZE = 12` unconditionally — Linux
x86_64 shape applied to every target.  Xorg/xserver uses libev, libev
uses epoll, and libev reads the struct using the compiler's natural
layout.  Every `epoll_wait` on arm64 returned garbage in `.data`,
crashing Xorg the moment the event loop looked at the first wakeup
source.

The fix:

```rust
#[cfg(target_arch = "x86_64")]
const EPOLL_EVENT_SIZE: usize = 12;
#[cfg(target_arch = "x86_64")]
const EPOLL_DATA_OFFSET: usize = 4;

#[cfg(not(target_arch = "x86_64"))]
const EPOLL_EVENT_SIZE: usize = 16;
#[cfg(not(target_arch = "x86_64"))]
const EPOLL_DATA_OFFSET: usize = 8;
```

And a matching fix in the lockfree path in `kernel/fs/epoll.rs`
(`collect_ready_inner` had its own hand-rolled 12-byte write).

## Fix 5: `WaitQueue` lost-wakeup race

Debugging the xsetroot hang that persists past Fix 4 turned up a race
in `kernel/process/wait_queue.rs::sleep_signalable_until`:

```rust
pub fn sleep_signalable_until<F, R>(&self, ...) -> Result<R> {
    // Fast path: condition already met — no queue ops at all.
    let fast = condition();
    match fast {
        Ok(Some(result)) => return Ok(result),
        Ok(None) => {}
    }
    // INNER:
    loop {
        // Check signals, enqueue, switch()...
        let mut q = self.queue.lock();
        q.push_back(current_process().clone());
        ...
        switch();
        // Re-check condition after waking.
        let result = condition();
        ...
    }
}
```

Classic lost-wakeup window: between the fast-path `condition()`
returning `None` and the enqueue, a concurrent writer can call
`wake_all()`, observe `waiter_count == 0`, do nothing, and leave the
sleeper waiting for a second wake that may never arrive.  Shape of
the hang: AF_UNIX `recvfrom` returns `EAGAIN`, then `ppoll` — and
`ppoll` never returns because the peer's write happened in the gap
between "buffer empty" and "we're on the wait queue."

Fix: re-check the condition AFTER enqueuing, BEFORE `switch()`.  Now
any write that landed after our initial fast-path check either
arrived before we enqueued (caught by the re-check) or after
(caught by the wake).

This resolves the lost-wakeup class of hangs but doesn't fix the
specific xsetroot repro we were chasing — that turned out to be a
different problem (Xorg entering a tight userspace loop servicing one
client without re-polling the listener; tracked as follow-up).  Still
worth landing independently — the race is real and affects every
AF_UNIX / pipe / epoll waiter.

## Fix 6: arm64 crash registers

Diagnosing Fix 4 required seeing the user register state at the
crash.  `CrashRegs` exists in `platform/crash_regs.rs` but is stashed
only from x86_64's interrupt handler — arm64 data aborts ran with an
empty stash and the SIGSEGV dump showed nothing.

Extended `platform/arm64/interrupt.rs::EC_DATA_ABORT_LOWER` to stash
x0..x15, SP_EL0, PC, SPSR, FAR_EL1 through the generic
`CrashRegs::stash` API, mapping arm64 registers into the x86-named
slots that the dump printer already handles.  Without this, spotting
`x0 = fault_addr - 0x10` across multiple runs wasn't possible.

## Result

Alpine aarch64 i3 test, HVF, 2 CPUs:

| before / after  | mount | xorg | i3 | xterm | pixels |
|---|---|---|---|---|---|
| blog 228 baseline | — (no target) | — | — | — | — |
| after Fix 1 | ✅ | fbdev: "No devices" | — | — | — |
| after Fix 2+3 | ✅ | open /dev/fb0 → SIGSEGV | — | — | — |
| after Fix 4 | ✅ | ✅ | ✅ | ✅ | ❌ |

**4/7 tests passing.**  Xorg, i3, xterm all run.  `i3status`,
`_NET_SUPPORTING_WM_CHECK`, and visible pixels are still failing —
tracked as follow-ups.

## Follow-ups

1. **`xsetroot` hangs.**  The lost-wakeup fix (Fix 5) alone didn't
   unblock this.  Per-PID strace + `collect_ready` instrumentation
   shows xsetroot does `writev(fd=3, 12)` (X11 setup request) →
   `recvfrom(fd=3, 8) = EAGAIN` → `ppoll(fd=3)` and waits forever.
   The listener poll on Xorg's abstract socket stops firing after the
   2nd accept (xdpyinfo + i3), even though later `connect()`s do push
   to the backlog and wake `POLL_WAIT_QUEUE`.  Xorg's own strace shows
   it in a tight userspace loop of `writev(fd=7) / recvmsg(fd=7)
   EAGAIN / setitimer / clock_gettime / writev(fd=7)` — no
   `epoll_pwait` between cycles.  Suggests libev's in-loop processing
   keeps fd=7 busy and never re-enters the event dispatch.  Needs
   deeper investigation of libev on arm64 or whatever's making
   recvmsg-EAGAIN not unblock the event loop.
2. **i3 never fires its `exec` autostart lines.**  Likely the same
   underlying issue as (1).
3. **ext2 doesn't flush data blocks on halt.**  The test writes a
   `fb-snapshot.bgra` to the mounted rootfs; the directory entry
   persists but the inode data blocks don't.  Needs an explicit fs
   sync in the shutdown path, or a virtio-console exfil channel.
4. **Screenshot pipeline.**  Ramfb via fw_cfg (at `0x9020000` on
   virt) would give us a QEMU-visible display without needing PCIe
   ECAM.  ~200 lines.
5. **Port these fixes to the x64 path too?** Fix 4 (`epoll_event`
   layout) is genuinely x86_64-only in the wild (packed layout is
   correct there), but it's worth checking whether any Kevlar code
   path accidentally truncates epoll events on x64 via the same code
   we just edited.  Quick audit pending.

## Files touched

```
tools/build-alpine-i3.py                (new, apko-based)
testing/test_i3.c                        (new)
tools/build-initramfs.py                 (arm64 desktop tests)
Makefile                                  (test-i3 / test-i3-smp / run-alpine-i3)
platform/address.rs                      (mmio_read/write{8,16,32,64})
platform/arm64/bootinfo.rs               (DTB virtio-mmio discovery)
platform/arm64/interrupt.rs              (crash_regs::stash)
libs/virtio/transports/virtio_mmio.rs    (mmio_* helpers)
exts/virtio_blk/lib.rs                   (HVF-safe probe)
exts/virtio_net/lib.rs                   (HVF-safe probe)
exts/bochs_fb/lib.rs                     (init_ram_backed)
kernel/main.rs                            (arm64 ram-fb wiring)
kernel/syscalls/epoll.rs                 (arch-dep EpollEvent layout)
kernel/fs/epoll.rs                       (arch-dep layout in lockfree path)
kernel/process/wait_queue.rs             (lost-wakeup: recheck after enqueue)
```

## Follow-up session: EPOLLET + ramfb scan-out

A subsequent session implemented the three remaining items from
the follow-ups above:

### EPOLLET on Unix sockets (fixed properly)

`UnixStream` and `UnixListener` got `state_gen` + `et_watcher_count`
and proper `poll_gen` / `notify_epoll_et` impls.  The first cut of
this regressed `xorg_running` (4/7 → 3/7) because `poll_gen()`
returned non-zero unconditionally — `kernel/fs/epoll.rs::poll_cached`
treats non-zero gen as "cache OK and serve cached poll status until
gen changes," but bumps were gated on `et_watcher_count > 0`, so
LT-only fds froze on a stale-empty cache.  Fix: return 0 from
`poll_gen` when no ET watchers exist, mirroring `kernel/pipe.rs`.
Doubled Xorg's accept count from 2 → 4 in repeated runs but didn't
fully unblock the i3 autostart issue (separate libev-internal
busy-loop on fd=7 — needs further investigation).

### ext2 sync-on-halt (partially fixed)

`kernel/process/process.rs:1322` (init-exit halt path) now calls
`kevlar_ext2::sync_all()` before `halt()`, mirroring the reboot
syscall.  The "Syncing filesystems before halt..." log fires.

But: extracted `fb-snapshot.bgra` files still come back size=0 from
the disk image.  Inode 2210 itself is "bad type" (mode=0).  The
inode-table block carrying it isn't being flushed despite sync_all
running.  Initial attempt added `flush_dirty()` at the end of
`flush_metadata` to push staged GDT writes — that hung the test
(deadlock or lock-order violation) and was reverted.  The real fix
needs a deeper look at the ext2 cache flushing order.

### ramfb scan-out (working end-to-end)

`exts/ramfb` (new crate) writes a 28-byte `RAMFBCfg` struct via
fw_cfg's DMA mode.  Initial port-mode byte writes via `mmio_write8`
silently failed under HVF — the fw_cfg file stayed all-zeros after
28 byte writes.  Switching to one DMA descriptor write to `fw_cfg
+ 0x10` worked: readback shows the bytes landed.

DTB walker in `platform/arm64/bootinfo.rs` extended to detect
`fw-cfg@*` nodes; `BootInfo` gained `fw_cfg_base: Option<PAddr>`.
`tools/run-qemu.py` got a `--display-vnc PORT` flag (replaces
`-nographic` with `-display vnc=:PORT -vga none` so ramfb actually
renders in batch mode) and now passes `-device ramfb` for arm64.

Result — first-ever Kevlar arm64 visible-display screenshot:

![Kevlar arm64 KEVLAR test pattern via ramfb](images/229-ramfb-kevlar-arm64.png)

The KEVLAR block-letter pattern was painted by `test_i3.c` with a
single `open("/dev/fb0", O_RDWR) + write()` — proving the full
scan-out path: kernel `alloc_pages` → ram-backed fb at paddr
`0x7d000000` → `/dev/fb0` device file (via `bochs_fb::init_ram_backed`)
→ ramfb config in fw_cfg → QEMU scans the paddr each frame → VNC
display backend → vncdotool snapshot.

### Phase-3-era files added/touched

```
exts/ramfb/{Cargo.toml, lib.rs}          (new crate)
platform/bootinfo.rs                     (BootInfo.fw_cfg_base)
platform/arm64/bootinfo.rs               (DTB fw-cfg walker)
platform/x64/bootinfo.rs                 (fw_cfg_base: None on x64)
kernel/main.rs                            (ramfb::init wiring)
kernel/Cargo.toml, Cargo.toml             (ramfb crate dep)
kernel/net/unix_socket.rs                (StreamSide refactor + EPOLLET)
kernel/process/process.rs                (sync_all() before halt)
services/kevlar_ext2/src/lib.rs          (flush_metadata note)
tools/run-qemu.py                         (--display-vnc + ramfb device + cache=writethrough)
testing/test_i3.c                        (KEVLAR test-pattern paint, hold-30s)
```

## Phase 4 (in progress): mouse-driven desktop

Goal: turn the visible-but-static screenshot into an interactive desktop
(LXDE-style: openbox + tint2 + pcmanfm).  Gating issue: the run on top of
ramfb logs `(EE) PreInit returned 2 for "<default pointer>"` — Xorg
can't init input.  QEMU `-machine virt` arm64 has no PS/2 or USB
without explicit `-device`s, and the natural input is virtio-mmio.

Done in this phase:

- **`exts/virtio_input` crate** — new probe-virtio-mmio driver that
  discovers `device_id=18` slots, sets up the eventq + statusq with
  feature negotiation `(features=0, num_virtqueues=2)`, pre-fills the
  eventq with 1024 8-byte writable buffers, and on IRQ pops 8-byte
  virtio-input events into a per-device `VecDeque` for userspace.
- **`kernel/fs/devfs/evdev.rs`** — `EvdevFile` implementing `FileLike`
  for `/dev/input/event0..3`.  Lazily resolves to the i-th
  `virtio_input::registered_devices()` entry.  Read returns 24-byte
  Linux `struct input_event` records (timeval + ty/code/value) with a
  CLOCK_MONOTONIC-derived timestamp.  Implements every evdev ioctl
  Xorg's xf86-input-evdev driver issues at probe: EVIOCGVERSION,
  EVIOCGID (BUS_VIRTUAL), EVIOCGNAME, EVIOCGPHYS/UNIQ/PROP,
  EVIOCGREP/SREP, EVIOCGKEY/LED/SND/SW (all-zero state),
  EVIOCGBIT(0..0x1f) (event-type and code bitmaps with EV_SYN/KEY/
  REL/ABS/MSC/REP set; mouse buttons in EV_KEY; REL_X/Y + REL_WHEEL
  in EV_REL; ABS_X/Y in EV_ABS), EVIOCGABS(axis) (input_absinfo with
  range 0..32767 for tablet-style abs axes), EVIOCGRAB.
- **`kernel/fs/sysfs.rs`** — `/sys/class/input/eventN/{dev,uevent,name}`
  populated for each registered virtio-input device, mirroring the
  `/sys/class/graphics/fb0` pattern from blog 229.
- **`kernel/fs/devfs/mod.rs`** — `/dev/input/event{0,1,2,3}` device
  files registered, all `EvdevFile` instances differentiated by index.
- **`tools/run-qemu.py`** — adds `-device virtio-keyboard-device` and
  `-device virtio-mouse-device` (`event_idx=off,indirect_desc=off`) for
  the arm64 default args.
- **`tools/build-alpine-i3.py`** — drops `xf86-input-libinput` (which
  needs udev) in favour of `xf86-input-evdev` and adds `xev`.
  Generates an explicit `/etc/X11/xorg.conf.d/20-input.conf` with
  static `Section "InputDevice" Driver "evdev" Option "Device"
  "/dev/input/event0"` (kb) and `event1` (ms), bound via
  ServerLayout.

Result so far: Xorg cleanly accepts `kb0` and `ms0` as XINPUT
devices, no more "PreInit returned 2" errors.  evdev driver
identifies the mouse with "Found 20 mouse buttons / Found scroll
wheel(s) / Found x and y relative axes / Found absolute axes /
Configuring as tablet".

### The "first IRQ silences the device" bug

Initial symptom: VNC mouse events reach QEMU successfully
(`vncdotool` confirms `CONNECTED → MOVES_SENT → CLICK_SENT`), but
the guest's IRQ handler fires exactly twice during boot
(`isr=0x1, popped=0`) and never again.  No mouse cursor, no
events on `/dev/input/event1`.

VNC was a red herring.  Replaced it with a `tools/qmp-input-probe.py`
harness that boots the kernel, opens a QMP unix socket, and uses
`{"execute":"input-send-event","arguments":{"events":[{"type":"btn",
"data":{"down":true,"button":"left"}}]}}` to inject events
directly into QEMU's input dispatcher — bypassing VNC entirely.
Same result: 2 init IRQs, then silence.

Then `x-query-virtio-status` + `x-query-virtio-queue-status` lit up
the actual state:

```
last-avail-idx: 7   used-idx: 7   shadow-avail-idx: 1024
avail.flags: 0      isr: 0
```

QEMU **did** consume 7 buffers and put 7 events in the used ring.
`avail.flags=0` means the driver is asking for interrupts.  But
`isr=0` and the guest's IRQ count was still stuck at 2.  Events were
landing; the IRQ just wasn't being delivered.

Three bugs, in increasing severity:

1. **`virtio-mmio` transport never wrote `InterruptACK` (0x64).**
   Per spec §4.2.2.1 the driver must read `InterruptStatus` (0x60)
   *and* write the bits back to `InterruptACK` to clear them.
   Without the ack, ISR stays latched and the device cannot raise a
   new edge on the same bit.  `read_isr_status()` now writes back
   the bits it read.  This was masked on virtio-blk (synchronous
   poll) and virtio-net (continuous traffic re-edges the line).

2. **`VirtQueue::new` never wrote `QueueNum` (0x38).**  Some QEMU
   versions default `QueueNum` to `num_default` (64 for
   virtio-input) which is smaller than `QueueNumMax` (1024).  The
   driver was registering 1024 buffers but the device was only
   tracking modulo 64 of them.  Now we explicitly write
   `set_queue_size(num_descs)` before `enable_queue()`.

3. **The actual showstopper:** `platform/arm64/interrupt.rs::arm64_handle_irq`
   was running `gic::disable_irq(other)` *before* dispatching to the
   driver handler — and never re-enabling.  A comment said this
   prevented "flooding from unhandled level-triggered interrupts" at
   boot, but the only IRQs that reach the `other` arm are ones that
   already had `attach_irq` called for them, so the flooding scenario
   doesn't exist in practice.  What it *did* do: silently kill the
   GIC SPI for the rest of the run after the first interrupt.  Every
   level-triggered virtio device only ever got one interrupt before
   its line was masked forever.  virtio-blk hid this by polling, and
   virtio-net hid it because TCP retransmits paper over lost RX
   IRQs.  virtio-input is sparse and unforgiving: one click → one
   IRQ → mask → silence.

   Fix: drop the `disable_irq` line.  The driver handler is
   responsible for quiescing the device (via the new ack path
   above), and that's enough — the GIC EOI lowers the pending bit,
   and the next ISR=1 transition cleanly re-fires.

After all three fixes:

```
virtio-input: irq #0 isr=0x1 popped=0   ← init, no events yet
virtio-input: irq #1 isr=0x1 popped=0
virtio-input: irq #2 isr=0x1 popped=2   ← BTN_LEFT down + SYN
virtio-input: irq #3 isr=0x1 popped=2   ← BTN_LEFT up + SYN
virtio-input: irq #4 isr=0x1 popped=3   ← REL X+Y + SYN
virtio-input: irq #5 isr=0x1 popped=3   ← key 'a' down/up + SYN
```

Files:

```
libs/virtio/transports/virtio_mmio.rs    (write InterruptACK in read_isr_status)
libs/virtio/device.rs                    (set_queue_size before enable_queue)
exts/virtio_input/lib.rs                 (negotiate VIRTIO_F_VERSION_1)
platform/arm64/interrupt.rs              (drop disable_irq before handler dispatch)
tools/qmp-input-probe.py                 (new — QMP-based IRQ flow probe)
```

### What was *not* the bug, in case you'd guessed it (I did)

The earlier "Xorg busy-loops on fd=7" hypothesis from the prior
session looked plausible, but turning on per-PID strace
(`strace-pid=4` on the kernel cmdline) showed Xorg is actually
calling `epoll_pwait` between every iteration — it isn't bypassing
the kernel.  The real downstream issue is **AF_UNIX listener
starvation**: Xorg accepts the first 3-4 connections (xdpyinfo, i3,
xterm) and then every later client (`xsetroot`, `xprop`) arrives
on the listener's backlog and waits.  The listener's poll() returns
POLLIN, but Xorg's main loop seems to keep coming back to the
client fd it last serviced rather than draining the listener's
queue.  Captured the data with two diagnostics worth keeping
around: `/proc/<pid>/fd/N` now resolves to the Linux convention
(`socket:[INODE]`, `pipe:[INODE]`, `anon_inode:[N]`) instead of `/`,
and the `qmp-input-probe.py` flow generalises into a "boot then
poke QEMU" harness.

Files for the procfs symlink improvement:

```
kernel/fs/procfs/proc_self.rs          (S_IFSOCK / S_IFIFO branch in lookup)
kernel/net/unix_socket.rs              (Stat with S_IFSOCK + per-socket inode_no)
testing/test_i3.c                      (snapshot Xorg fd table during xsetroot)
```

Everything else (mouse cursor visible in a desktop screenshot,
i3status spawning, `xsetroot -solid` actually painting `/dev/fb0`)
is downstream of the listener-starvation issue and waits for the
next session.
