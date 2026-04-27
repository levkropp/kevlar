# 256 — kABI K7: Linux-source compat headers

K7 lands.  A C source written *exactly* the way a Linux 6.12
hello-world module is written compiles in Kevlar's tree and
runs.  No `kevlar_kabi_*` references, no source-level
Kevlar-isms — just `<linux/init.h>`, `<linux/module.h>`,
`pr_info(KERN_INFO ...)`, `MODULE_LICENSE("GPL")`,
`module_init(fn)`.

The full source of the K7 demo:

```c
// SPDX-License-Identifier: GPL-2.0
#include <linux/init.h>
#include <linux/module.h>
#include <linux/kernel.h>

MODULE_LICENSE("GPL");
MODULE_AUTHOR("Kevlar");
MODULE_DESCRIPTION("kABI K7: Linux-source-shape hello-world module");
MODULE_VERSION("0.1");

static int __init k7_init(void)
{
	pr_info("k7: hello from a Linux-shape module v%d.%d\n", 1, 0);
	pr_info("k7: KERN_INFO + variadic printk works\n");
	return 0;
}

static void __exit k7_exit(void)
{
	pr_info("k7: goodbye\n");
}

module_init(k7_init);
module_exit(k7_exit);
```

This is the canonical shape of every Linux module from
`drivers/gpio/gpio-xilinx.c` to `samples/kobject/`.  The
output on serial:

```
kabi: loading /lib/modules/k7.ko
kabi: loaded /lib/modules/k7.ko (2336 bytes, 14 sections, 22 symbols)
kabi: /lib/modules/k7.ko license=Some("GPL") author=Some("Kevlar")
       desc=Some("kABI K7: Linux-source-shape hello-world module")
[mod] k7: hello from a Linux-shape module v1.0
[mod] k7: KERN_INFO + variadic printk works
kabi: k7 init_module returned 0
```

`make ARCH=arm64 test-module-k7` is the regression target.
All eight kABI tests now pass: K1 (loader), K2 (runtime), K3
(device model), K4 (file_operations), K5 (MMIO/DMA), K6
(variadic printk), K7 (Linux-source compat), and the K4
userspace fd test.

## Two compatibility layers

There's a useful distinction between *protocol*
compatibility and *binary* compatibility.

- **Protocol**: Linux-source-style C code compiles in our
  tree, against headers that look like Linux's, hits a
  loader that understands the resulting ELF, and runs.
  Module authors can write code in `vim drivers/foo.c` style.
- **Binary**: a `.ko` file produced by Linux's own build
  system, against Linux's actual headers, drops into our
  initramfs and loads.  No Kevlar-specific source paths.

K7 lands the first.  K8 lands the second.

These overlap at the *header level*: real Linux modules
include `<linux/...>`.  K7's compat headers re-export our
K2-K6 shims under matching include paths.  K8 will swap
those for Linux's actual headers — at which point the
header layouts must match Linux's exactly (and our shims
must understand structs that look like Linux's, not
Kevlar's).

## What K7 added

A small `testing/linux/` directory with seven headers, each
~50 lines:

```
testing/linux/
├── init.h        — module_init, module_exit, __init, __exit
├── module.h      — MODULE_LICENSE/AUTHOR/DESCRIPTION/VERSION
├── kernel.h      — container_of, ARRAY_SIZE, BUG, IS_ERR/PTR_ERR
├── printk.h      — KERN_LEVEL constants, pr_emerg..pr_debug
├── types.h       — u8/u16/u32/u64, s8/.../s64, size_t/ssize_t
├── compiler.h    — likely/unlikely, __maybe_unused, READ_ONCE
└── stddef.h      — NULL + offsetof
```

Plus the K7 demo module + a build hook + a Makefile target.

No new kernel-side Rust code.  The K6 surface was already
broad enough; K7 is a header-level rename layer.

## The two macros that make K7 work

**`module_init(fn)` → `init_module` alias.**  Linux 6.12's
expansion is:

```c
#define module_init(initfn)					\
	static inline initcall_t __maybe_unused __inittest(void)	\
	{ return initfn; }					\
	int init_module(void) __copy(initfn)			\
		__attribute__((alias(#initfn)));		\
	___ADDRESSABLE(init_module, __initdata);
```

K7's compat header drops the type-check + addressable
clauses (those reference Linux-internal types we haven't
provided) and keeps the load-bearing piece:

```c
#define module_init(initfn) \
    int init_module(void) __attribute__((alias(#initfn)))
```

GCC's `__attribute__((alias("initfn")))` creates an alias
symbol.  The user writes:

```c
static int __init k7_init(void) { ... }
module_init(k7_init);
```

After preprocessing + compile, the ELF symbol table contains:
- `k7_init` (T)
- `init_module` (T) — same address as `k7_init`

Kevlar's K1 loader looks up `init_module` by name and
calls it.  The alias works because the symbols share an
address.

**`pr_info(fmt, ...)` → `printk(KERN_INFO fmt, ...)`.**
Linux's `pr_info` macro prepends a `KERN_INFO` preamble
(literally the bytes `\x01` + `'6'`) to the format string,
then calls `printk`.  K7's compat header does exactly the
same:

```c
#define KERN_INFO "\x01" "6"
#define pr_info(fmt, ...)  printk(KERN_INFO fmt, ##__VA_ARGS__)
```

K6's variadic printk formatter already strips the
`\x01<digit>` preamble silently (see blog 255).  So
`pr_info("hello %d\n", 42)` expands to
`printk("\x01" "6" "hello %d\n", 42)`, the formatter sees
the SOH+digit, advances past, parses `"hello %d\n"` with
`42` from VaList, and emits `"hello 42"`.

Two macros + one variadic printk = the entire foundation
for "Linux modules' source code compiles."

## What got slightly hard

**Path ergonomics.**  GCC's `-I` flag adds a directory to
the include search path; `<linux/init.h>` then resolves
under each `-I` path.  My first build used
`-I testing/linux/`, which makes GCC search for
`testing/linux/linux/init.h` — wrong.  Fix: `-I testing/`,
which makes `linux/init.h` resolve to `testing/linux/init.h`.

**`bool` typedef collision.**  My `linux/types.h` had
`typedef _Bool bool;`.  `aarch64-linux-musl-gcc -ffreestanding`
hadn't included `<stdbool.h>`, but it *did* know about C23's
`bool` keyword — and rejects a `bool` typedef as a redefinition.
Fix: drop the typedef.  Hello-world modules don't need
`bool` anyway; modules that do can `#include <stdbool.h>` or
declare their own.

**IDE warnings, not real ones.**  VS Code's clang doesn't
know our `-I` paths and flags every `<linux/init.h>` line as
"file not found."  It also doesn't speak GCC's
`__attribute__((alias))` (Mach-O / Darwin doesn't support
aliases).  Both are lint noise — the real cross-compile
(`aarch64-linux-musl-gcc`) handles both correctly.

## What K7 didn't do

- **Compile against actual Linux headers from
  `build/linux-src/include/`.**  K7 uses Kevlar-specific
  compat headers under `testing/linux/`.  K8 swaps them for
  Linux's real headers.
- **Linux struct-layout exactness.**  Our
  `struct wait_queue_head` is 8 bytes (a single opaque
  pointer); Linux's is 24 bytes (`spinlock_t` + `list_head`).
  Modules that read fields directly through the Linux
  declarations would see garbage.  K7 modules don't read
  through these structs; they only call API functions.
  K8 reconciles.
- **`THIS_MODULE` resolution.**  K7's compat header stubs
  `THIS_MODULE` to `NULL`.  Modules that pass `THIS_MODULE`
  as an argument (most commonly `.owner = THIS_MODULE`)
  pass NULL, which our shims accept.
- **`__init` section migration.**  Real Linux moves
  `__init`-marked code into `.init.text` and frees it after
  module init completes.  K7 leaves `__init` as a no-op so
  the code stays in `.text` permanently — matches Kevlar's
  no-unload policy from K1-K6.
- **`vermagic` checking.**  Linux's loader rejects modules
  whose `vermagic` doesn't match.  Kevlar's loader doesn't
  check.  K7 modules don't emit a vermagic, since K7
  compiles against Kevlar's compat headers.  K8 may
  surface this when loading real Linux .ko binaries.
- **`%pK` / `%pf` / `%pe` printk modifiers.**  K6 deferred
  these; K7 didn't add them either.
- **The full include tree.**  Linux's `<linux/module.h>`
  alone pulls in 21 direct includes feeding 100+ transitive
  deps.  K7's `linux/module.h` is 50 lines because every
  K7-supported macro can be expressed without those deps.
  K8 hits the dep cliff.

## Cumulative kABI surface (K1-K7)

The exported-symbol table is unchanged from K6 (~85 entries).
K7 added zero kernel-side symbols — every K7 export is
already there from K1-K6, just under a Linux-style name in
the compat header.

What changed is the *vocabulary* a module author can use
to spell those calls:

| What the module writes | Resolves to |
|---|---|
| `pr_info(KERN_INFO "%d\n", v)` | `printk("\x01" "6" "%d\n", v)` |
| `MODULE_LICENSE("GPL")` | `__MODULE_INFO(license, "GPL")` → `.modinfo` bytes |
| `module_init(my_init)` | `init_module` alias |
| `container_of(p, type, member)` | offsetof arithmetic |
| `IS_ERR(p)` | range check |
| `__maybe_unused` | GCC `__attribute__((unused))` |

All of these are macros — no runtime cost, just
preprocessor expansion.  The `.ko` that comes out the
other side has the same code as a hand-written
Kevlar-shape module.

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
| K8 — binary compat: prebuilt Linux .ko loads | ⏳ next |
| K9 | ⏳ |

## What K8 looks like

K8 is the binary-compat milestone.  Two threads:

1. **Linux struct-layout exactness.**  Audit every K2-K6
   shim against Linux 6.12's actual UAPI header.
   `struct wait_queue_head`: bring it to 24 bytes with
   `spinlock_t` at offset 0, `list_head` at offset 8.
   `struct device`: hundreds of bytes; expose only the
   fields modules read directly at Linux's offsets, leave
   the rest as opaque-but-correctly-sized padding.  Same
   for `completion`, `work_struct`, `file_operations`,
   `file`, `inode`, `cdev`, `device_driver`, `bus_type`,
   `platform_device`, `platform_driver`.
2. **Compile a real Linux module against
   `build/linux-src/include/`.**  Run
   `make defconfig + make modules_prepare` in the linux-src
   tree (generates `include/generated/autoconf.h` and other
   build artifacts).  Then `aarch64-linux-musl-gcc -c
   -I build/linux-src/include
   -I build/linux-src/arch/arm64/include ...` against a
   hello-world `.c` file.  Watch the include-tree dep
   cliff.  Stub or define what's missing.  Get to a clean
   build.

The K8 demo target: a hello-world module compiled against
the actual Linux source tree (not Kevlar's compat
headers), produces a `.ko` byte-for-byte equivalent to
what Linux's own kbuild would produce, and loads through
the K1 loader.

After K8, the kABI compatibility claim becomes literal —
"Linux source code drops into Kevlar and runs" with no
quoting, no asterisks, no qualifiers.  The remaining work
(K9+) is breadth: more Linux exports, more bus types,
more driver classes, until enough surface exists to load
non-trivial real-world Linux modules.

K7 was the protocol; K8 is the proof.
