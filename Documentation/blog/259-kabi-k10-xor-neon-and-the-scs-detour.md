# 259 — kABI K10: xor-neon.ko, cpu_have_feature, and the SCS detour

K10 lands.  `xor-neon.ko` — Canonical's prebuilt arm64
NEON-accelerated XOR template (used by RAID 5/6 parity) —
loads in Kevlar and `init_module` returns 0.

xor-neon was supposed to be a small, predictable step
beyond K9: one undefined symbol (`cpu_have_feature`), one
Rust stub, ship.  It almost was.  But the first attempt
crashed inside HVF before xor-neon's init even reached the
function call we'd shimmed.  The crash revealed a load-
bearing prerequisite for **every other Ubuntu arm64
module**: shadow call stack (SCS) infrastructure.  K10
expanded to fix that too.

```
kabi: loading /lib/modules/xor-neon.ko (Ubuntu 26.04)
kabi: loaded /lib/modules/xor-neon.ko (11057 bytes, 40 sections, 61 symbols)
kabi: /lib/modules/xor-neon.ko license=Some("GPL") author=Some("Jackie Liu")
       desc=Some("ARMv8 XOR Extensions")
kabi: xor-neon init_module returned 0
```

`make ARCH=arm64 test-module-k10` passes.  All 11 kABI tests
green.

## The straightforward part: cpu_have_feature

xor-neon's one undefined symbol:

```sh
$ aarch64-linux-musl-nm xor-neon.ko | grep " U "
                 U cpu_have_feature
```

In Linux 6.x, `cpu_have_feature(num)` was a static-inline
in `<asm/cpufeature.h>` that read a per-cpu hwcaps bitmap
and returned a bool.  In 7.0 it became a real exported
function.  xor-neon calls it once (`cpu_have_feature(17)`
— HWCAP_ASIMD bit) to decide whether to register its
NEON-accelerated XOR template.

K10's shim:

```rust
// kernel/kabi/cpufeature.rs
#[unsafe(no_mangle)]
pub extern "C" fn cpu_have_feature(_num: u16) -> bool {
    true
}
ksym!(cpu_have_feature);
```

Stubbed to always-true.  Kevlar runs on QEMU virt arm64
with KVM passthrough where every common feature (NEON,
FP, atomics, crc32, sha) is present.  More precise
gating defers to whenever a driver actually misbehaves
because of an over-claimed feature; for now, "yes you
can" is the right answer.

## The unexpected detour: shadow call stacks

The first attempted load:

```
kabi: loaded /lib/modules/xor-neon.ko (11057 bytes, 40 sections, 61 symbols)
kabi: applied 29 relocations (1 trampoline(s))
Assertion failed: (isv), function hvf_handle_exception, file hvf.c, line 2181.
```

QEMU's HVF backend on macOS asserted on an exception with
`ESR_EL2.ISV=0` (Instruction Specific Syndrome Valid bit
clear).  ISV-clear means "trap with no decoded instruction
info" — usually a fault from a context HVF wasn't expecting.

Disassembling xor-neon's `init_module` showed why:

```
0000000000000008 <init_module>:
   8:  d503201f   nop
   c:  d503201f   nop
  10:  d503233f   paciasp
  14:  f800865e   str  x30, [x18], #8     ← here
  18:  52800220   mov  w0, #0x11
  ...
```

That `str x30, [x18], #8` is the **shadow call stack
(SCS)** prologue.  Linux's arm64 build with
`CONFIG_SHADOW_CALL_STACK=y` (Ubuntu's default) reserves
register `x18` as the SCS pointer.  Function entry pushes
LR onto the SCS:

```
str  x30, [x18], #8        ; *x18 = LR; x18 += 8
```

Function exit pops:

```
ldr  x30, [x18, #-8]!      ; x18 -= 8; LR = *x18
```

If the `x30` stored on the stack via `stp` is corrupted by
an attacker (ROP), the epilogue's SCS-loaded LR is the
authoritative one — RET goes to the SCS-saved address.
It's a CFI defense.

Linux maintains x18 as part of every kernel task's context.
**Kevlar doesn't.**  Our Rust kernel uses x18 only when the
ABI requires it (which on aarch64-Linux is "platform
register; no special meaning unless OS reserves it").  At
the moment we transmute `addr` to `extern "C" fn() -> i32`
and call it, x18 contains whatever Rust left there —
likely 0 or some garbage.

xor-neon's first instruction `str x30, [x18], #8` writes to
`*x18`.  If x18=0, that's a write to address 0 — a normal
data abort.  But Apple HVF's specific check is that the
trap-and-emulate path needs a valid syndrome to operate on,
and this particular fault apparently doesn't get one in the
hypervisor's view.  Hence the assertion.

The right fix is "make Linux's SCS contract hold for the
duration of the call."

## The fix

`LoadedModule::call_init()` — a new method that
allocates a 1KB SCS area, points x18 at it, calls
init_module via inline asm, and restores x18:

```rust
#[cfg(target_arch = "aarch64")]
fn call_module_init_with_scs(f: extern "C" fn() -> i32) -> i32 {
    let mut scs: Vec<u8> = alloc::vec![0u8; 1024];
    let scs_ptr = scs.as_mut_ptr();
    let result: i32;
    unsafe {
        core::arch::asm!(
            "mov x9, x18",         // save caller's x18
            "mov x18, {scs}",      // install SCS pointer
            "blr {fp}",            // call init_module
            "mov x18, x9",         // restore caller's x18
            scs = in(reg) scs_ptr,
            fp = in(reg) f,
            lateout("x0") result,
            out("x9") _,
            clobber_abi("C"),
        );
    }
    drop(scs);
    result
}
```

Why does the SCS area only need to live for the duration
of the call, not afterwards?  Because the function's
epilogue pops its own SCS push.  init_module enters with
some `x18` value, advances it on push, decrements it back
on pop.  By the time the epilogue's `ret` executes, x18 is
back to where we set it.  The Vec drops cleanly after the
asm block returns.

1024 bytes of SCS is what Linux uses per-task by default —
plenty for the call depth a typical init_module reaches.
If a future driver's init recurses deeper than that, we
hike the size; for now this is right.

## Why this is load-bearing for the whole kABI arc

It's worth sitting with this finding.  **Almost every
Ubuntu arm64 `.ko`** — every driver, every filesystem,
every protocol module — was built with SCS on.  K9's
bman-test happened to slip through because its
init_module body is empty (no SCS prologue gets emitted
for a function that never calls anything else).  The
moment we picked any module with a real call path, we'd
hit this.

Without K10's `call_init()` wrapper, the entire iterative
LinuxKPI ascent (K11+) was blocked.  Every Ubuntu binary
beyond bman-test would have crashed in the same place
before we could even diagnose what symbols it needed.

This is exactly the shape of work the LinuxKPI playbook
predicts: pick a target, attempt the load, find the
gnarliest *non-obvious* prerequisite, fix it, retry.
Sometimes it's a missing exported symbol.  Sometimes it's
a calling-convention assumption you didn't know was an
assumption.  K10 surfaced both in one milestone.

## What got cheaper for K11+

The 10 module-load sites in `kernel/main.rs` (one per
demo module from K1-K10) all changed from:

```rust
match m.init_fn {
    Some(f) => {
        let rc = f();
        info!("kabi: ... init_module returned {}", rc);
    }
    None => warn!("..."),
}
```

to:

```rust
match m.call_init() {
    Some(rc) => {
        info!("kabi: ... init_module returned {}", rc);
    }
    None => warn!("..."),
}
```

A bulk regex did the rewrite.  Future module-load sites
inherit the SCS-aware path automatically.

The other piece — `cpu_have_feature` — is now part of
the kABI surface (87 exports total) and any future
module that calls it just works.  This is the
compounding payoff of the LinuxKPI approach: each shim
added benefits every subsequent driver that needs it.

## What K10 didn't do

- **Real cpu feature gating.**  `cpu_have_feature` always
  returns true.  When K11+ surfaces a driver that needs
  truthful feature data (e.g. a crypto driver that picks
  between AES-NI and software paths based on a feature
  bit), we'll wire up reading from the actual ID
  registers.  For now: yes-to-everything is correct on
  KVM-enabled QEMU virt arm64.
- **PAC support.**  xor-neon also uses `paciasp` /
  `autiasp` for pointer authentication.  Those execute
  as NOPs when the kernel hasn't enabled the PAC keys
  in `SCTLR_EL1.EnIA` (Kevlar hasn't).  Works as no-ops
  for now.
- **NEON FPEN enablement.**  xor-neon's *registered*
  template would, if invoked, execute NEON instructions
  that need `CPACR_EL1.FPEN ≠ 0`.  Kevlar runs with
  FPEN restricted (Rust kernel uses `+soft-float`), so
  invoking the template would trap.  K10 doesn't invoke
  it (registration just records a function pointer);
  the trap deferred until something actually USES the
  template.

## Cumulative kABI surface (K1-K10)

87 exported symbols (K9's 86 + `cpu_have_feature`).
Plus the SCS-aware call wrapper (architectural change
to the loader, not an export).

## Status

| Surface | Status |
|---|---|
| K1-K9 | ✅ |
| K10 — xor-neon.ko + cpu_have_feature + SCS | ✅ |
| K11+ — richer drivers, more subsystems | ⏳ next |

## What K11 looks like

K11's target is `dummy.ko` — Linux's network dummy
device, 23 undefined symbols.  It registers a virtual
network interface that you can `ip link add dummy0
type dummy` and use as a packet sink.

23 undefs is a much bigger jump than K10's 1.  All 23
are network-stack functions: `register_netdevice`,
`alloc_netdev_mqs`, `ether_setup`, `eth_mac_addr`,
`free_netdev`, etc.  That's a whole subsystem worth of
kABI surface to stub.

K11's deliverable: the 23 stubs land, dummy.ko's
`init_module` registers a "dummy0" netdev (probably
not visible to userspace yet — Kevlar doesn't have a
sysfs `/sys/class/net/` tree wired to the kABI side),
and returns 0.

After K11, the network-subsystem stubs are in place —
which means many *other* network modules (dummy is
~2.7 KB; veth is ~6 KB; loopback is built-in;
specialized ones like dummy-irq are simpler) inherit
those exports.  Compounding payoff again: one
subsystem-shaped milestone makes the next ten modules
much easier.

After that — K12 toward input subsystem stubs,
K13-K15 toward fbdev, K16+ toward DRM and the actual
graphical Alpine boot.  K10 was the on-ramp.  The road
to graphical now stretches in front of us; each
milestone is one more driver class shimmed.
