# 257 — kABI K8: compiling against Ubuntu 26.04's Linux 7.0 headers

K8 lands.  A C source written *exactly* the way a Linux 7.0
driver is written compiles against **Ubuntu 26.04's actual
`linux-headers-7.0.0-14-generic` package** — Canonical's
prebuilt header tree, with `include/generated/{autoconf.h,
bounds.h, rq-offsets.h, asm-offsets.h, timeconst.h, vdso-
offsets.h, ...}` populated by their build farm — and runs in
Kevlar.

The K8 demo source:

```c
// SPDX-License-Identifier: GPL-2.0
#include <linux/init.h>
#include <linux/module.h>
#include <linux/kernel.h>

MODULE_LICENSE("GPL");
MODULE_AUTHOR("Kevlar");
MODULE_DESCRIPTION("kABI K8: Linux 7.0 / Ubuntu 26.04 headers hello-world");
MODULE_VERSION("0.1");

static int __init k8_init(void)
{
	pr_info("k8: hello from real Linux 7.0 headers (Ubuntu 26.04)\n");
	pr_info("k8: built against build/linux-src/include/\n");
	return 0;
}

static void __exit k8_exit(void) { pr_info("k8: goodbye\n"); }

module_init(k8_init);
module_exit(k8_exit);
```

The C preprocessor walks `linux/init.h` from Ubuntu's tree.
The output `.ko`'s code lives in `.init.text` / `.exit.text`
(Linux's `__init` / `__exit` section migration), the
`init_module` symbol aliases `k8_init` via `module_init()`,
and Kevlar's K1 loader runs the result.

Serial output, end-to-end:

```
kabi: loading /lib/modules/k8.ko
kabi: loaded /lib/modules/k8.ko (3224 bytes, 21 sections, 31 symbols)
kabi: /lib/modules/k8.ko license=Some("GPL") author=Some("Kevlar")
       desc=Some("kABI K8: Linux 7.0 / Ubuntu 26.04 headers hello-world")
[mod] k8: hello from real Linux 7.0 headers (Ubuntu 26.04)
[mod] k8: built against build/linux-src/include/
kabi: k8 init_module returned 0
```

`make ARCH=arm64 test-module-k8` is the new regression
target.  All nine kABI test targets now pass.

## The course-correction: vanilla v7.0 → Ubuntu 26.04

The session opened on the K7 plan: bump our `build/linux-src/`
from Linux 6.12 to 7.0.  My first pass cloned vanilla `v7.0`
from Linus Torvalds's tree and ran `make defconfig + make
modules_prepare` to generate the build artifacts hello-world
needs (`autoconf.h`, `bounds.h`, etc.).

`defconfig` worked.  `modules_prepare` didn't.

Apple clang on macOS chokes building Linux's host-side
modpost helper:

```
scripts/mod/file2alias.c:1248: error: member reference base type
  'unsigned char[16]' is not a structure or union
```

Linux's `struct tee_client_device_id` has a `uuid_t uuid` field
(`uuid_t` is `struct { __u8 b[16]; }` in 7.0).  The
`DEF_FIELD_ADDR` macro produces `typeof(...->uuid) *uuid`
which Apple clang reduces to `unsigned char (*)[16]` (the
underlying array), losing the wrapping struct.  Then
`uuid->b[N]` fails because plain arrays don't have `.b`.
GCC handles `typeof` differently here; Apple clang does not.

Without modpost, the prepare chain bails before generating
the cascade of `include/generated/*.h` files Linux's source
includes need.  My first attempt tried to stub each missing
generated header empirically — `include/generated/timeconst.h`
needs values, `include/generated/asm-offsets.h` needs values,
each derived from running actual kernel code at build time.
You can hand-tune values until things compile, but every
`static_assert(sizeof(struct ...) == ...)` in the chain
checks your work.

I surfaced the blocker.  The user steered: use Ubuntu 26.04's
prebuilt headers instead of vanilla.  That's closer to the
project's stated kABI target anyway (per the saved memory:
"Pinned target: Linux 7.0 / Ubuntu 26.04 LTS Resolute
Raccoon"), and it sidesteps the host-tool problem because
Canonical's build farm already ran `modules_prepare` on
their Linux build hosts and we just consume the output.

## The Ubuntu deb path

Ubuntu's kernel headers ship as two debs:

- `linux-headers-7.0.0-14_*_all.deb` (~14 MB) — the
  arch-independent source tree (Kbuild, scripts, full
  `include/linux/`, etc.)
- `linux-headers-7.0.0-14-generic_*_arm64.deb` (~42 MB) —
  the arch-specific overlay: `include/generated/*` populated,
  `arch/arm64/include/generated/asm/*` populated, plus
  `Module.symvers` and the matching `.config`.

Both packages extract via `ar x` (deb is an ar archive),
then `zstd -d data.tar.zst` (Ubuntu uses zstd compression
inside debs since 21.10), then `tar x`.

Merged into a single tree at `build/linux-src/`, the result
is exactly what an Ubuntu kernel-developer environment looks
like after `apt install linux-headers-generic`.

`tools/fetch-ubuntu-linux-headers.sh` automates the whole
sequence.  ~4 MB of script, ~250 MB on disk for the merged
tree.  Idempotent — exits early if `autoconf.h` + `bounds.h`
already exist.

## The two compile flags that mattered

Two non-obvious flags were needed to compile against Linux's
real headers on `aarch64-linux-musl-gcc`:

- **`-std=gnu11`**: musl-gcc defaulted to a newer C standard
  where `bool` and `false` are reserved keywords.  Linux's
  `<linux/types.h>` does `typedef _Bool bool;` and
  `<linux/stddef.h>` defines `false = 0` in an enum.  Both
  fail under C23+ rules.  Switching back to gnu11 quiets it.

- **`-fplan9-extensions`**: Linux's `struct filename`
  declaration uses an anonymous-struct-as-member without a
  field name:

  ```c
  struct filename {
      struct __filename_head;        /* anon embedded — Plan 9 ext */
      const char iname[EMBEDDED_NAME_MAX];
  };
  ```

  By default, GCC treats `struct __filename_head;` inside a
  struct as a forward declaration that doesn't add a member.
  `-fplan9-extensions` (a GCC-specific flag) interprets it
  as embedding the struct's fields directly, the way Linux
  intends.  Without it, `sizeof(struct filename)` is wrong
  and a `static_assert(sizeof(struct filename) % 64 == 0)`
  trips at compile time.

  Real Linux's kbuild Makefile passes
  `-fplan9-extensions` for every kernel-mode compile.  We
  needed to mirror.

## The one runtime symbol we hadn't thought about

K1-K7's `printk` was directly the symbol modules linked
against.  K8's first compile produced a `.ko` referencing
**`_printk`** (with leading underscore), not `printk`.  Why?

Linux 7.0's `<linux/printk.h>` line 511:

```c
#define printk(fmt, ...) printk_index_wrap(_printk, fmt, ##__VA_ARGS__)
```

`printk()` is a *macro* that wraps `_printk()` for the
printk-index machinery (build-time tracking of every printk
call site for runtime introspection).  The actual function
is `_printk`.

Two-line fix in `kernel/kabi/printk.rs`:

```rust
ksym!(printk);
crate::ksym_named!("_printk", printk);
```

Same Rust function exposed under both names.  K7 modules
(compiled against our compat headers) link against `printk`;
K8 modules (compiled against Ubuntu's headers) link against
`_printk`.  Both resolve to the same code.

## What K8 didn't do

Same disclaimers as the K7 blog, but specifically:

- **Linux struct-layout exactness still pending.**  The K8
  module only *calls* API functions (`printk`, `module_init`).
  It doesn't dereference Linux struct fields — so we never
  hit a layout mismatch.  When a future K9 module uses
  `DECLARE_WAIT_QUEUE_HEAD()` (which expects a 24-byte
  struct with `spinlock_t` at offset 0 and `struct list_head`
  at offset 8), our 8-byte `_kevlar_inner` shim would be
  catastrophically wrong.  K9 reconciles.
- **Loading prebuilt `.ko` binaries from Ubuntu's
  `linux-image-7.0.0-14-generic` package.**  K8 *compiles*
  against Ubuntu's headers; K9+ would *load* their compiled
  drivers (`virtio_net.ko`, `virtio_blk.ko`, etc.).  Many
  more Linux exports needed before that's feasible.
- **`make modules_prepare` still doesn't work locally.**
  Apple clang's `typeof` quirk on `scripts/mod/file2alias.c`
  hasn't been patched.  We sidestepped the problem by using
  Ubuntu's prebuilt artifacts; if a future change requires
  re-running `modules_prepare` (e.g., a `.config` swap), we'd
  need a Linux container or a host-tool patch.

## Cumulative kABI surface (K1-K8)

Functions: 86 (K7's 85 + `_printk` alias).  No new symbols,
just a re-export of `printk` under the name Linux 7.0's
header chain expects.

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
| Phase: replace Linux kernel with Kevlar in real workloads | ⏳ next |

## What's next

K1-K8 was eight milestones building up the kABI surface.
After K8, the natural pivot is to **stop adding new kABI
primitives and start using what we have**.  The plan:

1. **Busybox + bench, Kevlar as Linux drop-in.**  Existing
   Alpine + busybox stack, but boot on Kevlar instead of
   Linux.  Whatever Linux normally module-loads, Kevlar
   loads via the K1-K8 surface.  Compare boot times,
   bench results.
2. **systemd + Kevlar.**  systemd cares about a lot more
   kernel surface (cgroups v2, sysfs, /proc, tons of
   syscalls).  Validate our kABI is broad enough for it.
3. **Alpine text-mode boot on Kevlar.**  Real init,
   apk-installed packages, /etc/init.d.  No graphics yet.
4. **Alpine graphical boot on Kevlar.**  Xorg via Linux's
   modesetting driver (loaded as a `.ko`), input via
   `virtio_input.ko`, framebuffer via `simpledrm.ko` or
   `virtio_gpu.ko`.

Each rung exercises more of the kABI than the last.  The
gaps each surfaces are the next milestones — driven by what
the workload needs, not by our kABI roadmap guessing.

This is the inflection point in the kABI arc: the kABI
isn't done (K9 layout exactness, K10+ GPU drivers, etc.
are real work), but it's done *enough* that Linux source
recognizes it as Linux.  Time to find out what breaks when
real software shows up.
