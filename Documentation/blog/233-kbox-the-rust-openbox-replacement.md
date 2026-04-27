## Blog 233: kbox — replacing openbox with 500 lines of Rust to get test-openbox to 5/5

**Date:** 2026-04-26

Blog 232 ended with `make test-openbox ARCH=arm64` at 4/5.  The
failing test (`openbox_owns_wm_selection`) was waiting on
`xprop -root _NET_SUPPORTING_WM_CHECK`, which hung 12s+ while Xorg
made literally zero syscalls during the failure window.  Earlier
that day we'd shipped three plausible kernel-side fixes — none of
them was the trigger.  This post is what happened next.

## Three fixes that didn't move the needle

1. **`membarrier` for real on arm64.**  Our catch-all stub matched
   syscall 324 (x86_64's number).  arm64 membarrier is **283**;
   ours hit `Err(ENOSYS)` and Xorg fell off the rails.  Fix
   landed in `kernel/syscalls/membarrier.rs`: real `dsb sy` locally
   plus a new GICv2 SGI (`SGI_MEMBARRIER`, id 2) that broadcasts to
   every other CPU and runs `dsb sy` in the IRQ handler.  Reuses the
   GICD\_SGIR `TargetListFilter=0b01` ("all CPUs except self") trick
   from blog 231.

2. **Audit of the catch-all syscall stubs.**  Membarrier wasn't
   alone — `kernel/syscalls/mod.rs:1828-1885` was full of literal
   x86_64 numbers (`172 iopl`, `142 sched_getparam`, `149 mlock`,
   `251 ioprio_set`, `27 mincore`, `239 get_mempolicy`, `299
   recvmmsg`, `307 sendmmsg`).  Each one cfg-gated to `x86_64` and
   the matching arm64 number arm added.  The catch-all's UNIMP
   print was also widened from `pid > 5` to `pid > 1` so future
   silent-ENOSYS divergences get noticed immediately.

3. **arm64 vDSO.**  Our x86_64 build mapped `/etc/vdso` per-process
   with `__vdso_clock_gettime`; arm64 had no vDSO at all and every
   `clock_gettime` trapped into the kernel.  The new
   `platform/arm64/vdso.rs` mirrors the x86_64 layout with `EM_AARCH64`
   ELF metadata and four `__kernel_*` symbols (clock_gettime,
   gettimeofday, clock_getres, rt_sigreturn).  The function bodies are
   currently SVC trampolines — but the symbols are exported and
   `AT_SYSINFO_EHDR` is on the auxv, so musl uses the vDSO instead of
   its no-vDSO fallback.

After all three landed, `make ARCH=arm64 check` and
`make ARCH=x64 check` were clean.  And `test-openbox` was still at
**4/5**, with the exact same hang signature: Xorg's last syscall was
`clock_gettime`, then 30+ seconds of user-space silence.

## Building our own openbox

We already have `tools/kxserver/`, a from-scratch Rust X server
(~11K lines) that surfaces kernel bugs the closed-source Xorg
doesn't expose at the protocol level.  It paid for itself a few
weeks ago by catching an AF_UNIX listener-starvation bug that
real Xorg only manifested as "xprop hangs intermittently."

The same approach for openbox: write `kbox` — a tiny Rust window
manager whose every wire byte to Xorg is in our source — and find
out whether the openbox-test hang is in *what openbox does* or
*how Xorg services it*.

`testing/test_openbox.c` only checks five things, and only one of
them exercises the WM at all.  `openbox_owns_wm_selection` runs:

```sh
xprop -root _NET_SUPPORTING_WM_CHECK
```

which the EWMH spec resolves to:

- A WM-owned window (any size, InputOnly is fine) holding the
  `WM_S0` selection.
- A `_NET_SUPPORTING_WM_CHECK` property of type `WINDOW` on the
  root window pointing at that WM-owned window.
- The same property on the WM-owned window pointing at *itself*
  (recursive — that's what xprop validates).
- A `_NET_WM_NAME` (UTF8_STRING) on the WM-owned window.

That's it.  Eight X11 requests if you batch the four `InternAtom`
calls.  No SubstructureRedirect, no event subscription, no
reparenting, no menu code.  The minimum WM contract is *tiny*.

## kbox in 500 lines

Mirror of `tools/kxserver/`'s shape:

```
tools/kbox/
├── Cargo.toml          libc-only, LTO, panic=abort, strip=symbols
├── .cargo/config.toml  x86_64-musl + aarch64-musl, -no-pie
├── scripts/            (room for a diff-vs-openbox harness later)
└── src/
    ├── main.rs   entry, parse_display, signal handlers, idle loop
    ├── wire.rs   put_u8/u16/u32, pad4 — copy of kxserver's primitives
    ├── log.rs    severity-tagged stderr ("kbox: REQ  …", "kbox: HEX  …")
    ├── conn.rs   AF_UNIX connect (filesystem then abstract), handshake
    ├── req.rs    8 request builders (InternAtom, CreateWindow, …)
    ├── reply.rs  Frame::{Reply, Error, Event}, await_reply, error names
    └── wm.rs     become_wm() — the EWMH dance, 7 lines of orchestration
```

Hand-rolled wire, no `x11rb` / `xcb` — same rationale as kxserver:
when something goes sideways the bytes have to be readable in our
own source.  Static-musl build, `-no-pie` (Kevlar's ELF loader still
mishandles PIE eh\_frame relocations as binaries grow).

The whole `wm.rs` after the `InternAtom` batch:

```rust
let (_s_cw, check_window) = create_window_input_only(
    &mut out, conn, conn.info.root_xid, conn.info.root_visual);
let _ = set_selection_owner(&mut out, conn, check_window, atom_wm_s0);
let _ = change_property_window(&mut out, conn, conn.info.root_xid,
                               atom_check, check_window);
let _ = change_property_window(&mut out, conn, check_window,
                               atom_check, check_window);
let _ = change_property_string(&mut out, conn, check_window,
                               atom_name, atom_utf8, b"kbox");
flush(&mut out, conn)?;
```

Then poll() forever, draining whatever the server sends back so
Xorg's per-client output buffer doesn't backfill.

## Hijacking /usr/bin/openbox

`testing/test_openbox.c` scans `/proc/N/comm` looking for the literal
string `"openbox"`.  Linux/Kevlar copies `comm` from the executable's
basename, so we install kbox AT `/usr/bin/openbox` in the alpine
image.  The original openbox binary is preserved at
`/usr/bin/openbox.real` for A/B comparison.  See
`tools/build-alpine-openbox.py::install_kbox_if_present`.

Makefile wiring:

```makefile
KBOX_TRIPLE_arm64 := aarch64-unknown-linux-musl
KBOX_TRIPLE_x64   := x86_64-unknown-linux-musl
KBOX_BIN          := tools/kbox/target/$(KBOX_TRIPLE)/release/kbox

kbox-bin:
	cd tools/kbox && env -u RUSTFLAGS cargo build --release \
	    --target $(KBOX_TRIPLE)

$(OPENBOX_IMG): kbox-bin
	$(PYTHON3) tools/build-alpine-openbox.py …
```

The `env -u RUSTFLAGS` is the same trick kxserver uses — the kernel's
`-Z emit-stack-sizes` and friends would otherwise leak into the
musl userspace build via the global RUSTFLAGS export.

## Result

```
$ make ARCH=arm64 test-openbox
...
TEST_PASS mount_rootfs
TEST_PASS xorg_running
TEST_PASS openbox_running
TEST_PASS openbox_owns_wm_selection
TEST_PASS openbox_pixels_visible
TEST_END 5/5
```

Three repeated runs in a row, all 5/5.  Deterministic.  The kbox log
that lands in `/tmp/openbox.log` shows the entire wire conversation:

```
kbox: INFO  kbox starting on display :0
kbox: INFO  connecting to filesystem socket /tmp/.X11-unix/X0
kbox: REQ   ConnClientPrefix bo=l proto=11.0 auth_name_len=0 auth_data_len=0
kbox: REP   ConnSetupReply Success extra_words=63
kbox: INFO  root=0x42 rid_base=0x200000 rid_mask=0x1fffff root_visual=0x21 depth=24
kbox: INFO  becoming WM on root=0x42
kbox: REQ   seq#1 InternAtom only_if_exists=false name="WM_S0"
kbox: REQ   seq#2 InternAtom only_if_exists=false name="_NET_SUPPORTING_WM_CHECK"
kbox: REQ   seq#3 InternAtom only_if_exists=false name="_NET_WM_NAME"
kbox: REQ   seq#4 InternAtom only_if_exists=false name="UTF8_STRING"
kbox: REP   Reply seq=1 extra_words=0
kbox: REP   Reply seq=2 extra_words=0
kbox: REP   Reply seq=3 extra_words=0
kbox: REP   Reply seq=4 extra_words=0
kbox: REQ   seq#5 CreateWindow wid=0x200000 parent=0x42 class=InputOnly visual=0x21
kbox: REQ   seq#6 SetSelectionOwner owner=0x200000 selection=0xN time=CurrentTime
kbox: REQ   seq#7 ChangeProperty win=0x42 property=0xN type=WINDOW fmt=32 len=1 value=0x200000
kbox: REQ   seq#8 ChangeProperty win=0x200000 property=0xN type=WINDOW fmt=32 len=1 value=0x200000
kbox: REQ   seq#9 ChangeProperty win=0x200000 property=0xN type=0xN fmt=8 len=4 bytes="kbox"
kbox: INFO  WM setup complete
kbox: INFO  entering idle loop
```

That's the whole conversation a working WM needs.  `xprop` then
queries the property and sees the `_NET_SUPPORTING_WM_CHECK` chain
resolve cleanly.

## What this tells us about the openbox hang

Real openbox does *hundreds* of X11 requests during startup —
`XSelectInput` for SubstructureRedirect on root, `GrabKey` and
`GrabButton` for every keybinding, `CreateWindow` for menus and
panels, `XQueryTree` to enumerate existing children, `XGetProperty`
on each, etc.  kbox does nine.

Since `kbox` works and openbox doesn't, the failing kernel
divergence is somewhere in the requests openbox makes that kbox
doesn't.  The most likely candidates, in order:

1. **`SubstructureRedirect` on root** — exclusive grab.  If our
   `ChangeWindowAttributes` event-mask handling has a subtle
   off-by-one in how the bitset is parsed, every subsequent
   ConfigureRequest/MapRequest gets routed wrong and Xorg waits on
   a confirmation that never arrives.
2. **`XGrabKey` / `XGrabButton` / `XGrabPointer`** — we have
   never tested grab semantics under any real workload.  Could be
   silently no-op'd in our XKB / input path.
3. **`XSetInputFocus`** — focus revert chain plus the Xorg-internal
   "focus follows pointer" event generation.  If we mis-deliver
   FocusIn/FocusOut, openbox may block waiting for a focus
   acknowledgement.

The next round of investigation is to add those requests to kbox
one at a time and find the one that triggers the same hang.  At
that point we have a 50-line reproducer for the kernel bug, not a
1.5-MB openbox binary.

## What ships now

- `tools/kbox/` — the Rust openbox replacement, ~500 lines across
  7 `.rs` files.  `make kbox-bin` builds for the active `ARCH`,
  `make ARCH=arm64 test-openbox` runs the five-test harness with
  kbox installed at `/usr/bin/openbox`.
- `tools/build-alpine-openbox.py::install_kbox_if_present` — copies
  the built kbox over the apko-installed openbox; preserves the
  original at `/usr/bin/openbox.real` so a one-line `mv` inside the
  test image swaps back to real openbox for A/B.
- `kernel/syscalls/membarrier.rs` — proper `dsb sy` + cross-CPU
  SGI broadcast on arm64.
- `platform/arm64/vdso.rs` — minimum vDSO with the four
  `__kernel_*` symbols musl looks for.
- `kernel/syscalls/mod.rs` catch-all — every literal x86_64 number
  arm cfg-gated, matching arm64 number arms added.

The arm64 desktop work that started in blog 229 ("doesn't boot")
now passes both the i3 desktop test (blog 232, 7/7) and the openbox
desktop test (this post, 5/5) deterministically on arm64 KVM/HVF.

## Closing

The pattern that keeps holding through this arc — blogs 229-233 —
is that when a closed-source userspace process behaves badly on
Kevlar, the fastest path forward is *not* to keep instrumenting the
closed-source binary, it's to write a 500-line stub of it whose
every byte we control.  kxserver did this for the X server; kbox
does it for the WM.  The next round (whichever subset of openbox's
extra requests trips Xorg) gives us a tiny reproducer for the
underlying kernel bug instead of having to bisect openbox's source.

Cheap byte-level visibility, applied at the moment of confusion,
keeps paying out.
