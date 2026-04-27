# 255 — kABI K6: variadic printk + userspace fd test

K6 lands.  Two threads, both about closing protocol gaps with
real Linux:

1. **`printk` is variadic now.**  Loaded modules can call
   `printk("loaded foo v%d.%d (id=0x%x)\n", 1, 2, 0xcafe)` and
   the format string parses the way every Linux driver expects.
2. **Real userspace validates K4.**  A userspace test binary
   boots as PID 1, calls `open("/dev/k4-demo", O_RDONLY)` +
   `read(fd, ...)` through actual syscalls, and reads the bytes
   the K4 module's `read` fop returns.

End-to-end on serial:

```
[mod] [k6] decimal: 42
[mod] [k6] negative: -7
[mod] [k6] unsigned: 4294967290
[mod] [k6] hex: cafebabe
[mod] [k6] HEX: DEADBEEF
[mod] [k6] string: world
[mod] [k6] char: A
[mod] [k6] pointer: 0xffff000040000000
[mod] [k6] padded: 00042
[mod] [k6] mixed: answer = 42 (0x2a)
[mod] [k6] percent: 100%
[mod] [k6] init done
kabi: k6 init_module returned 0

USERSPACE: starting
USERSPACE: open ok
USERSPACE: read=hello from k4
USERSPACE: done
PID 1 exiting with status 0
```

`make ARCH=arm64 test-module-k6` and
`make ARCH=arm64 test-userspace-kabi` are the two new
regression targets.  All seven kABI tests now pass.

## Why both pieces in one milestone

K1-K5 left two specific protocol mismatches with Linux:

- **K1's printk shim ignored `%`-tokens.**  The K1 plan
  explicitly deferred this: "Variadic ABI is not stable in
  Rust; K1 prints the format string verbatim, ignoring
  `%`-tokens.  Sufficient for the hello-world demo.  K2
  swaps in a Linux-shaped variadic..."  K2 didn't.  K3-K5
  didn't either.  By K6, every kABI demo module has been
  using `printk("[k%d] ... \n")` style strings — and
  watching its own `%` literals print as `%` instead of
  numbers.  Time to fix it.
- **K4's char-device path was tested kernel-side only.**
  `kabi::cdev::read_dev_for_test` calls `FileLike::read`
  directly from `main.rs`.  The actual
  `sys_openat → ftable → FileLike::read` chain — what
  userspace actually exercises — wasn't validated.

Both are gating concerns for any future binary-Linux-module
work.  No prebuilt .ko will pass through K1-K5 if printk
prints garbage; and any divergence between kernel-context
and userspace-context FileLike calls would surface only
when real software hits the K4 surface.

K6 fixes both.

## Variadic printk

The Rust side took three pieces.

**Feature gate** (`kernel/main.rs`):

```rust
#![feature(c_variadic)]
```

`c_variadic` is nightly-only and lets a Rust function declare
itself with C's variadic ABI:

```rust
#[unsafe(no_mangle)]
pub unsafe extern "C" fn printk(fmt: *const c_char, mut args: ...) -> i32 {
    /* ... */
}
```

The `mut args: ...` is the syntactic gate.  Inside, `args`
is a `core::ffi::VaList<'_>`; `args.arg::<T>()` is the
equivalent of C's `va_arg(args, T)`.

**The formatter** (`kernel/kabi/printk_fmt.rs`, ~330 LOC):
walks the format string byte-by-byte.  Plain bytes copy
through; `%` triggers a spec parser.  Each conversion
(`d`, `u`, `x`, `s`, `p`, `c`, `%`) consumes the next
arg of the right type from `VaList`:

```rust
match conv {
    b'd' | b'i' => {
        let v: i64 = unsafe { read_signed(&spec, args) };
        emit_signed(sink, v, &spec);
    }
    b'u' => {
        let v: u64 = unsafe { read_unsigned(&spec, args) };
        emit_unsigned_dec(sink, v, &spec);
    }
    /* ... */
}
```

`read_signed`/`read_unsigned` honor length modifiers
(`%hd`, `%hhd`, `%ld`, `%lld`, `%zd`) by reading the right
underlying width from `args` and sign- or zero-extending:

```rust
unsafe fn read_signed(spec: &Spec, args: &mut VaList<'_>) -> i64 {
    unsafe {
        match spec.length {
            LenMod::Hh      => args.arg::<i32>() as i8  as i64,
            LenMod::H       => args.arg::<i32>() as i16 as i64,
            LenMod::L | LenMod::Ll | LenMod::Z => args.arg::<i64>(),
            LenMod::Default => args.arg::<i32>() as i64,
        }
    }
}
```

(Note: per the C `va_arg` ABI, `int` and `short`/`char`
all promote to `int` when passed variadically; we read
`i32` and then narrow.  `long`/`long long`/`size_t` are
read as `i64` directly.)

**Sink** (`Sink` in the same file): a fixed 1 KB byte
buffer with bounds-safe `push`.  After the format walk
completes, the buffer is decoded as UTF-8 and emitted via
Kevlar's existing `info!()` macro tagged `[mod]`.

**Linux KERN_LEVEL preamble**: real Linux modules write
`printk(KERN_INFO "loaded\n")` where `KERN_INFO` expands
to a 3-byte `\x01<digit>` prefix.  The formatter strips it
silently when present:

```rust
let first = *p;
if first == 0x01 {
    p = p.add(1);
    let lvl = *p;
    if lvl >= b'0' && lvl <= b'7' {
        p = p.add(1);
    }
}
```

**Pointer-type modifier scrubbing**: `%p` in modern Linux
kernels takes optional letter modifiers (`%pK` for
restricted kernel pointer, `%pf` for function symbol,
`%pe` for errno name).  K6 doesn't implement those — but
it does **silently consume** the trailing letters so a
module that does `printk("hashed=%pK\n", p)` doesn't print
a literal `K` after the pointer:

```rust
b'p' => {
    let v: usize = unsafe { args.arg::<usize>() };
    while *p != 0 && (*p).is_ascii_alphabetic() {
        p = p.add(1);
    }
    emit_hex(sink, v as u64, &spec, false, true);
}
```

K7+ implements the actual modifiers when modules need them.

## The userspace test

A 30-line C binary:

```c
int main(void) {
    write(1, "USERSPACE: starting\n", 20);

    int fd = open("/dev/k4-demo", O_RDONLY);
    if (fd < 0) { write(1, "USERSPACE: open failed\n", 23); return 1; }
    write(1, "USERSPACE: open ok\n", 19);

    char buf[64];
    ssize_t n = read(fd, buf, sizeof(buf) - 1);
    if (n < 0) { write(1, "USERSPACE: read failed\n", 23); close(fd); return 1; }
    buf[n] = 0;
    write(1, "USERSPACE: read=", 16);
    write(1, buf, n);

    close(fd);
    write(1, "USERSPACE: done\n", 16);
    return 0;
}
```

Cross-compiled with `aarch64-linux-musl-gcc -static
-no-pie` via Kevlar's existing `compile_all_local_arm64`
pipeline (one new line in the jobs list).  Output binary
gets installed at `/usr/bin/test-kabi-userspace` in the
initramfs cpio.

`make ARCH=arm64 test-userspace-kabi` rebuilds the kernel
with `INIT_SCRIPT=/usr/bin/test-kabi-userspace` (Kevlar's
compile-time init-path mechanism), boots, and greps the
serial log for the four markers.

Why a kernel rebuild instead of a runtime cmdline?  ARM64
QEMU virt boots Kevlar as an ELF `-kernel` image and
doesn't pass a DTB; without DTB, Kevlar's bootinfo parser
falls back to defaults and never sees QEMU's `-append`
string.  The compile-time `INIT_SCRIPT` path is the
supported workaround for now; K7+ may add ARM64 cmdline
ingestion via a synthetic DTB or a different mechanism.

## What got harder than expected

One sharp edge: the K2 module-side header had declared:

```c
extern void printk(const char *fmt);
```

— matching K1's non-variadic signature.  K6's k6-module.c
uses real format strings, so it needs the variadic shape:

```c
extern int printk(const char *fmt, ...);
```

But all of K2-K5's demo modules `#include
"kevlar_kabi_k2.h"` and inherit the type.  Re-declaring
`printk` as variadic in the header conflicts with the
old declaration in the same translation unit.

Fix: change the header's declaration to variadic.  The
C call sites in K2-K5 demos (`printk("[k2] init begin\n")`)
work unchanged under both signatures — single-arg call
to a variadic function is fine.  And the underlying
implementation is the same Rust function for all callers.
One-line header change unblocked everything.

## What K6 didn't do

- **`%pK` / `%pf` / `%pe` / `%ph` / `%pV`** — Linux's
  exotic pointer modifiers (kernel pointer obfuscation,
  symbol resolution, errno name resolution, hex dumps,
  nested `va_format`).  K6 silently strips the trailing
  letters and prints plain hex.  K7+ implements when
  needed.
- **Floating-point conversions** — Kevlar doesn't save FPU
  state across context switches; kernels don't print
  floats; `%f`/`%e`/`%g` are deliberately skipped.
- **`vsprintf` / `vsnprintf` / `snprintf` exports.**  The
  format-string parser is structured to be reusable when
  these surface (`format_into(sink, fmt, args)` is the
  reusable core).
- **`%n`** — writes the count of bytes printed back through
  a pointer arg.  Notoriously dangerous, rare in kernel.
- **Real ARM64 cmdline ingestion via QEMU `-append`.**  The
  test target rebuilds the kernel with `INIT_SCRIPT` env;
  K7+ may add synthetic-DTB or other path.
- **Linux struct-layout exactness.**  K2-K6 still use
  opaque `_kevlar_inner` shims at offsets we chose.

## Cumulative kABI surface (K1-K6)

The exported-symbol table is unchanged from K5 (~85
entries) — K6 didn't add new symbols, only fixed the
semantics of an existing one.  But every prior demo
module's `printk("[k%d] ...\n")` calls now work the way a
human would expect them to.

## Status

| Surface | Status |
|---|---|
| K1 — ELF .ko loader | ✅ |
| K2 — kmalloc / wait / work / completion | ✅ |
| K3 — device model + platform bind/probe | ✅ |
| K4 — file_operations + char-device | ✅ |
| K5 — ioremap + MMIO + DMA | ✅ |
| K6 — variadic printk + userspace fd test | ✅ |
| K7 — Linux struct-layout exactness + first prebuilt module | ⏳ next |
| K8-K9 | ⏳ |

## What K7 looks like

K7 is the milestone where Kevlar accepts a **prebuilt Linux
binary `.ko`** unchanged.

K1-K6 built every primitive a Linux driver expects, but
each one with a Kevlar-shape header and an opaque
`_kevlar_inner` slot.  Real Linux modules compile against
`<linux/wait.h>`, `<linux/device.h>`, etc. — headers that
declare `struct wait_queue_head` as 24 bytes with a
`spinlock_t` at offset 0 and a `list_head` at offset 8.
A binary `.ko` reads those fields directly.

K7 reconciles.  Two threads:

1. **Layout exactness.**  Each K2-K5 shim struct gets
   audited against the Linux 7.0 UAPI header for the same
   type.  Where a module reads a field directly (not
   through a kABI shim function), the field has to live at
   the Linux offset, with the Linux size and type.  We
   keep our `_kevlar_inner` slot but reposition it to a
   field offset modules don't read.
2. **First binary-Linux-module load.**  Compile a tiny
   "hello, world" module against the actual
   `/Users/neo/kevlar/build/linux-src/include/` headers
   using `aarch64-linux-musl-gcc -I` (no Linux kbuild —
   just headers + the C source + ld), and load it through
   the K1 loader.

The demo target: a Linux source file like
`testing/k7-real-linux.c` that uses `module_init()`,
`MODULE_LICENSE()`, and `printk(KERN_INFO ...)` exactly
as a real Linux 7.0 module would.  We compile against
the actual Linux UAPI headers.  The output `.ko` loads
through the K1 loader, the `init_module` symbol resolves,
and serial shows the module's printk output.

That's the inflection point — the moment "drop-in Linux
kernel for binary modules" stops being a claim about the
kABI surface and becomes a claim verified by an actual
binary in the boot log.
