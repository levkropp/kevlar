# 260 — kABI K11: dummy.ko, 23 network stubs, and the fixed-x18 detour

K11 lands.  Ubuntu's `dummy.ko` — Linux's standard "network dummy
device" virtual driver — loads in Kevlar and `init_module`
returns 0.

```
kabi: loading /lib/modules/dummy.ko (Ubuntu 26.04)
kabi: loaded /lib/modules/dummy.ko (16233 bytes, 43 sections, 90 symbols)
kabi: /lib/modules/dummy.ko license=Some("GPL")
       desc=Some("Dummy netdevice driver which discards all packets sent to it")
kabi: rtnl_link_register (stub)
kabi: dummy init_module returned 0
```

The straightforward part: 23 undefined symbols across rtnl,
netdev, ethtool, and skb spaces, all stubbed in one new file
(`kernel/kabi/net.rs`).  The detour: another arm64 ABI assumption
that took most of the session to diagnose.  This post covers
both.

`make ARCH=arm64 test-module-k11` is the new regression target.
12 kABI tests now pass.

## The straightforward part: 23 stubs

Inspecting `dummy.ko`'s undefined symbols:

```
$ aarch64-linux-musl-nm dummy.ko | grep " U "
                 U __stack_chk_fail
                 U alloc_netdev_mqs
                 U alt_cb_patch_nops
                 U consume_skb
                 U dev_addr_mod
                 U dev_lstats_read
                 U dynamic_cond_resched
                 U eth_mac_addr
                 U eth_validate_addr
                 U ether_setup
                 U ethtool_op_get_ts_info
                 U free_netdev
                 U get_random_bytes
                 U netif_carrier_off
                 U netif_carrier_on
                 U param_ops_int
                 U register_netdevice
                 U rtnl_link_register
                 U rtnl_link_unregister
                 U rtnl_lock
                 U rtnl_unlock
                 U skb_clone_tx_timestamp
                 U skb_tstamp_tx
```

Disassembling `init_module`'s default-numdummies path showed the
actual call graph: `rtnl_link_register → rtnl_lock → (per-device
loop with numdummies=1: alloc_netdev_mqs → register_netdevice →
dynamic_cond_resched) → rtnl_unlock → return`.  Six functions on
the hot path.  The other 17 are referenced from `dummy_link_ops`
callbacks (`dummy_xmit`, `dummy_dev_init`, etc.) that fire only
when something *else* invokes them — and in K11, nothing does.

So the stubs split into:
- **Active stubs** (called during init): `rtnl_link_register`
  logs + returns 0; `rtnl_lock`/`rtnl_unlock` are no-ops;
  `alloc_netdev_mqs` returns a 4 KB zeroed buffer (Linux's
  `struct net_device` is ~3 KB; we oversize so direct field
  writes like `dev->rtnl_link_ops` at offset 2328 stay
  in-bounds); `register_netdevice` returns 0; `dynamic_cond_resched`
  is no-op (more on this below).
- **Linker-resolved stubs** (referenced but never called from
  init): the other 17 — `eth_mac_addr`, `consume_skb`, etc. —
  return whatever Linux expects on success and do nothing.

Plus three infrastructure-shaped pieces that came up alongside:
- `param_ops_int` — Linux module-parameter machinery (`module_param(numdummies, int, 0)` records `&param_ops_int` in its
  `__param` section).  It's a *static struct*, not a function.
  Exported via `ksym_static!`.
- `__stack_chk_fail` + `__stack_chk_guard` — stack-protector
  references.  Guard sentinel constant, fail panics.
- `alt_cb_patch_nops` — arm64 alternative-instruction patcher.
  No-op; if a module's correctness depends on patching, K12+
  surfaces it.

20 functions + 1 static + 2 globals = a single subsystem
landing in one file.  ~150 LOC.  Mechanical work.

## The detour: x18 isn't actually preserved across Rust kernel calls

The first attempted load:

```
kabi: rtnl_link_register (stub)
kabi: rtnl_lock
kabi: alloc_netdev_mqs -> 0xffff00007fc2e010
kabi: register_netdevice
kabi: rtnl_unlock
[PANIC] CPU=0 at platform/arm64/interrupt.rs:127
panicked at platform/arm64/interrupt.rs:127:17:
kernel page fault: pc=0xffff00007d1d8afc, far=0x1fa, esr=0x96000021
```

After `rtnl_unlock` returns, control flows back to `init_module`,
which... immediately faults.  far=0x1fa = 506 — nowhere near a
valid kernel address.

Decoding the panic:

- ESR_EL1 = `0x96000021`
  - EC = bits[31:26] = `0x25` = data abort, same EL.
  - DFSC = bits[5:0] = `0x21` = translation fault, level 1.
- FAR = `0x1fa` — the faulting virtual address.
- PC = `0xffff00007d1d8afc` — inside the loaded module.

Mapping PC to source: dummy.ko was loaded at `0xffff00007d1d8000`,
so the faulting offset is `0xafc`.  Walking the section layout,
`.init.text` starts at image offset `0xa30`; offset within
`.init.text` is `0xcc`; `init_module` symbol is at
`.init.text+0x8`, so the fault is at `init_module + 0xc4`.

Disassembly of init_module:

```
0xc4: bl  0 <dynamic_cond_resched>      ← image+0xaf4
0xc8: b   0x50 <init_module+0x48>        ← image+0xaf8
0xcc: ldr x30, [x18, #-8]!              ← image+0xafc  ← FAULT
```

`0xcc` is the **shadow call stack pop** — the SCS epilogue.
`x18` was supposed to be the SCS pointer, set by our K10 wrapper
to a 1 KB heap allocation before calling init_module.

But `far=0x1fa` means the load address `[x18, #-8]` evaluated to
`0x1fa`, so `x18` was `0x202` at the moment of the load.  That's
not the SCS pointer we set up.  Something clobbered x18 between
init_module's prologue (`str x30, [x18], #8`) and its epilogue.

The K10 SCS wrapper:

```rust
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
```

This worked for `xor-neon.ko` in K10 because xor-neon's
`init_module` does the SCS push, calls `cpu_have_feature`
(returns true), then immediately runs the SCS pop and returns.
No other function calls.  x18 stays as `scs_ptr+8` throughout.

`dummy.ko`'s init_module is different.  It calls our shim
functions multiple times: `rtnl_link_register`, `rtnl_lock`,
`alloc_netdev_mqs`, `register_netdevice`, `dynamic_cond_resched`,
`rtnl_unlock`, etc.  Each call is a `bl` to a Rust `extern "C"`
function.

**Rust's compiler doesn't reserve x18 by default.**  On
aarch64-Linux, x18 is the "platform register" — not in the
standard caller-saved or callee-saved set.  Linux's kernel build
passes `-ffixed-x18` to gcc, which makes the compiler treat x18
as reserved (never allocated by the register allocator).

We hadn't done that in Kevlar.  So Rust's register allocator,
inside our `rtnl_link_register` (or any other shim), was free to
allocate x18 as a scratch register — clobbering our SCS pointer.
By the time `init_module`'s epilogue runs `ldr x30, [x18, #-8]!`,
x18 holds whatever Rust last wrote there: a small value like
`0x202`.

## The fix: -Z fixed-x18

Rust's nightly has the same flag, spelled `-Z fixed-x18`.  One
line in the Makefile:

```make
export RUSTFLAGS = -Z emit-stack-sizes $(if $(filter arm64,$(ARCH)),-Z fixed-x18,)
```

The whole kernel rebuilds.  Now Rust's register allocator never
touches x18.  Our SCS pointer survives across nested function
calls.

The K11 dummy.ko load on the rebuild:

```
kabi: rtnl_link_register (stub)
kabi: rtnl_lock
kabi: alloc_netdev_mqs -> 0xffff00007fc2e010
kabi: register_netdevice
kabi: rtnl_unlock
kabi: dummy init_module returned 0
```

Six logs, each from a stub firing.  Then the SCS pop succeeds.
Then init_module returns 0.

## What this fix earns us

`-Z fixed-x18` benefits **every future Ubuntu arm64 module
load**, not just dummy.  K9's bman-test happened to work because
its init_module is empty (no SCS prologue is emitted for an
empty function).  K10's xor-neon worked because there were no
nested calls between SCS push and pop.  Every subsequent module
that does *both* — has a prologue+epilogue and calls into our
shims — would have hit this same fault.

Now they don't.  The kABI's arm64 calling-convention contract
matches Linux's exactly: x18 is reserved.  All 90+ kABI exports
inherit the fix transparently.

## What's interesting about this class of bug

K10's SCS detour and K11's fixed-x18 detour are both
calling-convention compat issues — not missing symbols, not
struct layouts, not relocations.  They're "Linux assumes the
calling kernel speaks ABI dialect X; we speak Y; cross-call
breaks subtly."

K10 was: Linux assumes x18 is set up to a valid SCS area.
K11 was: Linux assumes x18 is preserved across calls.

Both are easy in retrospect, hard up front.  Each takes one
careful diagnosis pass to find — *the actual instructions
executing aren't where you'd guess from the high-level call
graph*.  PC=0xafc looked like the bl, but it was actually the
SCS pop two instructions later.  The true root cause is one
abstraction layer below the visible failure mode.

I expect K12+ to surface a few more of these as we hit each
new subsystem.  Examples not yet hit:
- **PAC keys** — Ubuntu modules use `paciasp` / `autiasp`.
  These execute as NOPs while `SCTLR_EL1.EnIA=0`.  When we
  eventually enable PAC for security reasons, key-mismatch
  becomes a thing.
- **NEON FPEN** — modules with NEON code (xor-neon's
  registered template, future DRM blit paths) need
  `CPACR_EL1.FPEN ≠ 0` at the EL where they execute.
- **MTE tags** — memory tagging extension is on for some
  Ubuntu builds; Kevlar's allocator doesn't tag.

Each will be a similar shape: load a module, hit a confusing
fault, find the ABI assumption that diverges, fix at the kernel
layer, every subsequent module benefits.

## Cumulative kABI surface (K1-K11)

108 exported symbols (K10's 87 + 21 K11 functions).  Plus the
two infrastructure pieces (`-Z fixed-x18` build flag; SCS
wrapper from K10).

## What K11 didn't do

- **Functional networking.**  dummy.ko's init returns 0 but
  there's no `/sys/class/net/dummy0` for userspace to find.
  `alloc_netdev_mqs` returns a 4 KB zeroed buffer with no
  field meanings; the dev struct is opaque to our shims.  K12+
  (or much later) wires real netdev registration.
- **Linux struct layout exactness for struct net_device.**
  Same — opaque buffer.  When a future driver's callback
  reads `dev->stats` or similar, layouts matter.
- **Real RTNL semantics.**  `rtnl_lock`/`rtnl_unlock` are
  no-ops.  Linux's RTNL serializes against concurrent netdev
  changes.  Single-threaded module init: no real concurrency.

## Status

| Surface | Status |
|---|---|
| K1-K10 | ✅ |
| K11 — dummy.ko + 23 network stubs + fixed-x18 | ✅ |
| K12+ — input subsystem (toward graphical Alpine keyboard/mouse) | ⏳ next |

## What K12 looks like

K12 picks an input-class Ubuntu module — probably `evdev.ko`
or `virtio_input.ko`.  Both are the kind of "userspace gets to
read keyboard/mouse events" infrastructure that any graphical
session needs.

Expected shape: a similar handful of stubs (input subsystem:
`input_register_device`, `input_event`,
`input_set_capability`, etc.), plus possibly some struct
layout work since input drivers do read fields off
`struct input_dev`.  Total: probably similar scale to K11's
network batch.

After K12 + K11 together, two whole driver classes (network +
input) are loadable as Ubuntu binaries.  K13-K15 then
tackles the framebuffer / DRM stack, which is the longest
remaining mile to graphical Alpine.
