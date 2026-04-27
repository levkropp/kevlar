# 258 — kABI K9: the first prebuilt Ubuntu binary runs in Kevlar

K9 lands.  A `.ko` extracted byte-for-byte from
`linux-modules-7.0.0-14-generic_7.0.0-14.14_arm64.deb` —
Canonical's actual Ubuntu 26.04 kernel module package —
loads in Kevlar and its `init_module` returns 0.

This is the inflection point.  K1-K8 built up the kABI
*surface*: a runtime that progressively absorbed more of
Linux's protocol (loader, allocator, device model, char-
device, MMIO/DMA, variadic printk, real headers).  K9 is
the moment where that surface gets validated against a
binary the project doesn't author and didn't compile —
just downloaded from a kernel.org mirror — and it runs.

The serial output:

```
kabi: loading /lib/modules/bman-test.ko (Ubuntu 26.04)
kabi: loaded /lib/modules/bman-test.ko (6465 bytes, 30 sections, 33 symbols)
kabi: /lib/modules/bman-test.ko license=Some("Dual BSD/GPL")
       author=Some("Geoff Thorpe") desc=Some("BMan testing")
kabi: bman-test init_module returned 0
```

`make ARCH=arm64 test-module-k9` is the new regression
target.  All 10 kABI tests now pass: K1-K8 + the K4
userspace fd test + K9.

## The shift in approach

K7 was: write a Linux-source-style `.c` file, compile against
*our* compat headers (`testing/linux/`), load it.  Protocol
match.

K8 was: write a Linux-source-style `.c` file, compile against
*Ubuntu's* actual headers, load it.  Source-side
faithfulness.

K9 is: don't write the source.  Don't compile.  Take the
`.ko` Canonical's build farm produced — bytes — and load it.

The shape of work changes accordingly.  K1-K8 were "implement
shims, validate by writing a demo that uses them."  K9
onwards is "pick a binary, attempt to load, diagnose what's
missing, shim around it, retry" — the Linuxulator/LinuxKPI
playbook.  Each subsequent milestone is driven by what a
specific binary needs, not by a pre-designed roadmap.

## The fetch path

Ubuntu's kernel module package is `linux-modules-
7.0.0-14-generic_7.0.0-14.14_arm64.deb` from
`mirrors.kernel.org/ubuntu/pool/main/l/linux/`.  ~285 MB
download, takes about 5 minutes on a typical connection.

Inside, the structure is:

```
debian-binary
control.tar
data.tar          ← actual module files
```

(Note: the modules deb uses plain `data.tar` whereas the
headers deb uses `data.tar.zst`.  Different compression
choices for different packages.  Our fetch script handles
both.)

`tar -xf data.tar` produces `usr/lib/modules/7.0.0-14-generic/
kernel/` containing 8538 `.ko.zst` files — Linux's modern
modules are zstd-compressed at rest.  The Linux kernel
loader decompresses on load; ours doesn't.

We decompress at *fetch* time:

```sh
find ... -name '*.ko.zst' | while read f; do
  zstd -d -q -f "$f" -o "${f%.zst}"
  rm "$f"
done
```

8538 decompressions, ~30 seconds.  The result: a directory
of plain ELF `.ko` files we can hand to our K1 loader
unchanged.

`tools/fetch-ubuntu-linux-modules.sh` automates this
sequence.  Idempotent — exits early if
`build/linux-modules/` is already populated.

## Picking the target

8538 modules.  Which one is the right K9 target?

The answer is "the smallest binary with `init_module`
defined and the fewest undefined symbol references."  The
fewer undefs, the fewer Linux exports we need to shim
before the load succeeds.

A scan:

```sh
for f in $(find ... -name "*.ko" -size -5k); do
  if nm "$f" | grep -q " T init_module"; then
    nundef=$(nm "$f" | grep -c " U ")
    sz=$(stat -f "%z" "$f")
    echo "$nundef $sz $f"
  fi
done | sort -n | head
```

Output:

```
0 6465 ...drivers/soc/fsl/qbman/bman-test.ko
1 11057 ...arch/arm64/lib/xor-neon.ko: cpu_have_feature
2 10081 ...drivers/media/rc/keymaps/rc-imon-pad.ko: rc_map_register, rc_map_unregister
2 10129 ...drivers/media/rc/keymaps/rc-nec-terratec-cinergy-xs.ko
2 10201 ...net/netfilter/xt_CLASSIFY.ko: xt_register_targets, xt_unregister_targets
...
```

**`bman-test.ko` has 0 undefined symbols.**  Its
`init_module` body, disassembled:

```
0000000000000028 <init_module>:
  28:  d503201f   nop
  2c:  d503201f   nop
  30:  52800000   mov  w0, #0x0
  34:  d65f03c0   ret
```

That's it.  Two ftrace `nop`s (preamble for runtime
patching), then `return 0`.  Pure init function.  No I/O.
No subsystem registration.  No struct dereferences.  No
external symbol references.

This is the bman-test driver from Freescale's QBMan
(buffer manager) test infrastructure.  In a non-QBMan
build, all the `bman_*` test functions are stubbed out
to nothing — a vestigial test harness preserved for
ABI compatibility.  Perfect K9 target.

## What K1's loader handled correctly without changes

K1 was designed in Sep 2025 around a hand-compiled
hello-world.  K9 throws a real Ubuntu binary at it, with
30 sections vs hello.ko's 13.  And the loader needed
zero changes.

bman-test's section list:

```
__ksymtab                       — exports (empty here)
__kcrctab                       — symbol version CRCs (CONFIG_MODVERSIONS=n: empty)
__patchable_function_entries    — ftrace patchable-entry markers
.text                           — code (init/exit just placeholders here)
.bss                            — zero-init data
.data                           — initialized data
.plt                            — placeholder (1 byte; modpost lazy-fills)
.init.plt                       — placeholder (1 byte)
.text.ftrace_trampoline         — placeholder (1 byte)
.init.text.ftrace_trampoline    — placeholder (1 byte)
.note.GNU-stack                 — note
.init.text                      — actual init_module body (where we resolve)
.exit.text                      — actual cleanup_module body
.exit.data                      — pointer to cleanup_module
.init.data                      — pointer to init_module
.modinfo                        — license / author / vermagic / srcversion
.comment                        — compiler info
.note.gnu.property              — GNU note
.note.gnu.build-id              — build ID hash
.gnu.linkonce.this_module       — embedded struct module instance (tail-trimmed)
.note.Linux                     — Linux note
.ARM.attributes                 — ARM ELF attributes
.BTF                            — BPF Type Format debug info
```

K1's loader applies a simple rule: any section with
SHF_ALLOC gets memory-mapped + relocated; everything else
gets ignored.  That rule sufficed.  The relocations
`__patchable_function_entries` references (R_AARCH64_ABS64
to `.text`) get applied correctly because our relocator
already understood ABS64.  The init/exit `.init.text` /
`.exit.text` migrations are loaded as plain text since
they have SHF_ALLOC + SHF_EXEC.  K1 doesn't need to
distinguish.

The `.gnu.linkonce.this_module` section embeds a 1280-byte
`struct module` instance.  `init_module`'s symbol table
entry resolves the address (offset within `.init.text`) —
which the K1 loader's "look up the entry symbol by name"
machinery handles directly.  No knowledge of `struct
module`'s 50+ fields was required.

That's the architecturally satisfying part.  K1's design
five months ago — "load whatever has SHF_ALLOC, apply
relocations, find init_module by name" — was correct
enough that it absorbed a real Ubuntu binary on first run.

## What's a vermagic anyway

bman-test's `.modinfo` includes:

```
vermagic=7.0.0-14-generic SMP preempt mod_unload aarch64
```

Linux's loader treats this as a hard gate: if the running
kernel's vermagic doesn't match the module's, `insmod`
fails.  This is how Linux prevents a 6.12 module from
loading into a 7.0 kernel.

Kevlar isn't Linux.  Our vermagic check would say
"Kevlar 0.something" vs "7.0.0-14-generic" and reject
everything.  Instead, K1's `.modinfo` parser logs vermagic
and ignores the mismatch.  Intentional divergence: we know
we're loading binaries built for a different kernel; that's
the whole point.

## The path forward

K1-K9 was the runway.  K10+ is the ascent toward graphical
Alpine.  Each subsequent milestone picks a slightly more
ambitious Ubuntu module:

- **K10**: a module with ~3-5 simple undefined symbols.
  Likely `sm4_generic.ko` (4 crypto undefs) or a simple
  TCP congestion-control module (5 undefs into the TCP
  layer).  Add the relevant Linux exports as no-op or
  thin shims.  First milestone where new kABI exports get
  added because a real binary needs them.
- **K11-K12**: char-device drivers from Ubuntu's package
  (~50-100 undefs into VFS).  Layout-match `struct
  file_operations` (41 fields × 8 bytes = 328 bytes vs
  our current 8-byte shim).
- **K13-K14**: `simplefb.ko` or `simpledrm.ko` — first
  graphics-relevant driver.  fb_info / drm_device layouts.
- **K15-K16**: `virtio_input.ko` and `virtio_blk.ko` from
  Ubuntu, replacing our Rust `exts/virtio_*` drivers
  one-by-one.
- **K17+**: Xorg + i3 + LXDE on Kevlar, with all kernel-
  side device drivers being Linux's actual binaries.

That's the LinuxKPI playbook applied to a Rust microkernel.
Several months of work.  The compounding payoff: once each
driver class works, all the *other* drivers in that class
inherit our shim layer — virtio_net.ko works for free
once virtio core is shimmed; usb-storage works once USB
core is shimmed; etc.

## Cumulative kABI surface (K1-K9)

86 functions exported, unchanged from K8.  K9 added zero
new exports — the goal was to verify the existing surface
absorbs a real binary, not to extend it.  K10 will be where
new exports start landing in proportion to specific
drivers' needs.

## Status

| Surface | Status |
|---|---|
| K1 — ELF .ko loader | ✅ |
| K2 — kmalloc / wait / work / completion | ✅ |
| K3 — device model + platform bind/probe | ✅ |
| K4 — file_operations + char-device | ✅ |
| K5 — ioremap + MMIO + DMA | ✅ |
| K6 — variadic printk + userspace fd test | ✅ |
| K7 — Linux-source compat headers | ✅ |
| K8 — Ubuntu 26.04 / Linux 7.0 real headers | ✅ |
| K9 — first prebuilt Ubuntu .ko binary loads | ✅ |
| K10+ — drivers with real exports, layout exactness, ascent toward graphical | ⏳ |
