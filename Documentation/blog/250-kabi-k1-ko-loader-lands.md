# 250 — kABI K1: Linux `.ko` modules now load on Kevlar

K1 lands.  Kevlar can parse a Linux relocatable ELF object,
lay its sections out in kernel memory, resolve its undefined
symbols against an in-kernel exports table, apply aarch64
relocations, and call into the entry function.

The proof is a 20-line `hello-module.c` that calls a kernel-
exported `printk` and returns 0:

```
kabi: loading /lib/modules/hello.ko
kabi: loaded /lib/modules/hello.ko (1576 bytes, 13 sections, 14 symbols)
kabi: image layout: 4208 bytes (2 pages) at 0xffff00007efe2000
kabi: applied 3 relocations (1 trampoline(s))
[mod] hello from module!
kabi: my_init returned 0
```

Two lines of kernel-side logging sandwich the loaded module's
own output.  That last line — `kabi: my_init returned 0` —
proves control returned cleanly through the relocator-rewritten
call sites without crashing.

`make ARCH=arm64 test-module` is the regression target.  60
seconds end-to-end on QEMU+KVM.

## Pipeline

The loader (`kernel/kabi/loader.rs`) runs five stages on
`/lib/modules/hello.ko` after reading it out of the initramfs:

1. **Parse.**  `RelObj::parse` (`kernel/kabi/elf.rs`) wraps
   goblin's section-header / symtab / rela accessors.  Rejects
   anything that isn't `ET_REL` + `EM_AARCH64` with `ENOEXEC`.
2. **Layout.**  Walk SHF_ALLOC sections in order, align by
   `sh_addralign`, sum into a per-section offset map.  Reserve
   a 4 KB stub area at the end (more on this below).
3. **Allocate + copy.**  `alloc_pages(KERNEL)` for the total
   page count, then `copy_nonoverlapping` for SHT_PROGBITS,
   `write_bytes(0)` for SHT_NOBITS.
4. **Relocate.**  For each `.rela.<section>` entry, resolve the
   symbol (`kernel/kabi/symbols.rs` — handles SHN_UNDEF against
   the kernel exports table, normal section indices against the
   in-image layout), then call into the arch dispatcher.
5. **I-cache sync.**  `dsb ish; ic ialluis; dsb ish; isb` — the
   pages we just wrote contain new instructions; this flushes
   them globally before transferring control.

Then walk the symbol table to find `my_init` and call it.  K1
hardcodes the entry symbol name; K2 will honor Linux's
`module_init()` macro.

## Kernel symbol exports

Kernel-side functions callable from a module mark themselves
exportable with `ksym!(name)`:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn printk(fmt: *const c_char) { /* ... */ }
ksym!(printk);
```

The macro emits a `KSym { name: "printk", addr: <fn ptr> }`
into the linker section `.ksymtab`.  The arm64/x64 linker
scripts brace it with boundary symbols:

```
. = ALIGN(8);
__ksymtab_start = .;
KEEP(*(.ksymtab));
__ksymtab_end = .;
```

`KEEP` blocks `--gc-sections` from dropping unreferenced
entries.  `#[used]` on the static does the same job inside
the compiler.  At runtime, `kabi::exports::all()` materializes
a slice from `from_raw_parts(__ksymtab_start, len)` and
`lookup(name)` is a linear scan.  K1 has exactly one entry
(`printk`); K2 will sort + binary-search once the surface
hits hundreds.

## The interesting bug: CALL26 reach

aarch64 `bl` instructions encode a PC-relative branch in a
26-bit signed immediate (after `<<2`).  That's ±128 MB of
reach.  Plenty within a single binary, where the `bl` and its
target are both in `.text`.

But when the module is loaded into kernel memory, the `bl
printk` lives in a buddy-allocated page from `alloc_pages`,
and `printk` lives in the kernel's own `.text` near the top
of the kernel image.  On a 1 GB QEMU arm64 guest, those end
up about a gigabyte apart:

```
panicked at kernel/kabi/arch/arm64.rs:65:17:
kabi: R_AARCH64_CALL26/JUMP26 out of ±128MB range
  (sym=0xffff00004014d408 target=0xffff00007ef6c00c off=-1054993412)
```

That's the first thing the loader hit on its first run.  Off
by ~1 GB — eight times CALL26's reach.

The Linux kernel-module loader has solved this.  The standard
fix is a **PLT-style trampoline**: a tiny stub allocated near
the loaded module that performs an absolute branch via a
register, accessible from the module via a normal in-range
`bl`.  16 bytes, three words:

```
ldr  x16, [pc, #8]     ; 0x58000050
br   x16               ; 0xd61f0200
.quad <absolute target>
```

`x16` (IP0) is the AAPCS64 intra-procedure-call scratch
register — the procedure-call standard explicitly permits the
linker (and us) to clobber it without preserving it across
calls.  A `bl` landing on a PLT stub is indistinguishable
from a direct `bl` to the final target, modulo the extra
indirection.

The loader's stub area is 4 KB (256 stub slots — vastly more
than any single module needs).  When the relocator hits a
CALL26 / JUMP26 reloc, it range-checks; if the direct branch
won't reach, it lazily materializes a stub for that target
symbol, caches it (one stub per unique target across the
module), and rewrites the CALL26 to jump into the stub
instead.

For `hello.ko` that's 1 stub for `printk`.  For a real
driver it'll be a few dozen.  The stub area sizing is fine
for K2-class modules; if K7 hits the cap, the warning is
clear in the log and the cap bumps.

## Relocations supported in K1

aarch64-linux-musl-gcc with `-fno-pic -mcmodel=tiny` emits a
small set:

| Type | Encoding | Used for |
|---|---|---|
| `R_AARCH64_NONE` | nothing | filler |
| `R_AARCH64_ABS{32,64}` | data write | symbol pointers in data |
| `R_AARCH64_PREL{32,64}` | data write | `.eh_frame` PC-relative refs |
| `R_AARCH64_CALL26` / `JUMP26` | `bl` / `b` imm26 | external function calls |
| `R_AARCH64_ADR_PREL_LO21` | `adr` imm21 | tiny-model local label refs |
| `R_AARCH64_ADR_PREL_PG_HI21` | `adrp` imm21 | medium-model page-of-symbol |
| `R_AARCH64_ADD_ABS_LO12_NC` | `add` imm12 | low 12 bits of symbol |
| `R_AARCH64_LDST{8,16,32,64,128}_ABS_LO12_NC` | imm12 (scaled) | indirect loads |

Each gets its own arm of `kernel/kabi/arch/arm64.rs::apply()`.
Anything else **panics with the type number in the message**:

```rust
_ => panic!(
    "kabi: unhandled aarch64 reloc R_AARCH64_{} at module+{:#x} \
     (sym={:#x}, addend={})",
    r_type, target, sym_va, addend
),
```

This is the K1 contract.  Modules surface new relocation
kinds; the panic names them; we extend the match arm.  K1
covers the cases the toolchain emits today; K2 will hit
`R_AARCH64_GOT_*` when modules grow large enough to need a
GOT, and `R_AARCH64_TLS*` if anything lands with TLS.  No
guessing in advance.

`hello.ko` ended up needing exactly three: `ADR_PREL_LO21`
(load the address of `"hello from module!\n"`), `CALL26`
(jump to `printk`), and `PREL32` (a `.eh_frame` self-
reference).  The first attempt was missing `ADR_PREL_LO21`
because the plan only listed `ADR_PREL_PG_HI21` from the
medium memory model — `-mcmodel=tiny` uses the single-
instruction tiny variant for local references.  Added the
handler in the same iteration.

## What didn't make K1

- **`module_init()` / `module_exit()` macros.**  K1 takes the
  entry-symbol name as a string parameter to `load_module`.
  Real Linux modules wrap their init function with a macro
  that emits a `.modinfo` entry naming it.  Parsing
  `.modinfo` and `.gnu.linkonce.this_module` is K2's first
  surface.
- **Module unload.**  No `delete_module(2)`.  K1 modules live
  forever.
- **W^X.**  The boot direct map on arm64 is RWX-permissive,
  which is what makes `alloc_pages` + `copy + reloc` + `call`
  work without explicit page-table fiddling.  Hardening this
  to RW-during-reloc, RX-during-execution is K2 work.
- **x86_64.**  `kernel/kabi/arch/x64.rs` is a single
  `unimplemented!()` panic.  arm64 is the LXDE+desktop path;
  x86_64 modules aren't on the K1 critical path.  K2 will
  port — relocations are entirely different (no PLT needed
  in the small code model since the kernel is below 2 GB on
  x86_64, but R_X86_64_PC32 handling is still required).
- **Variadic printk.**  Rust doesn't have stable variadic
  ABI yet.  The K1 `printk` shim takes only `(*const c_char)`
  and prints the format string verbatim, ignoring `%`-tokens.
  K2 swaps in a tiny inline printf-class formatter.

## Files

| File | Purpose |
|---|---|
| `kernel/kabi/mod.rs` | module surface |
| `kernel/kabi/elf.rs` | goblin-backed ET_REL parser |
| `kernel/kabi/loader.rs` | layout, copy, relocate, find init |
| `kernel/kabi/symbols.rs` | resolve internal + external |
| `kernel/kabi/exports.rs` | `KSym` + `ksym!` + `all()` |
| `kernel/kabi/printk.rs` | `extern "C" fn printk` + `ksym!` |
| `kernel/kabi/reloc.rs` | arch-neutral dispatcher |
| `kernel/kabi/arch/arm64.rs` | aarch64 reloc handlers |
| `kernel/kabi/arch/x64.rs` | x86_64 stub (`unimplemented!`) |
| `kernel/arch/arm64/arm64.ld` | `__ksymtab_start/_end` clause |
| `kernel/arch/x64/x64.ld` | mirror clause |
| `platform/arm64/mod.rs` | `sync_icache_range` (dsb/ic/dsb/isb) |
| `testing/hello-module.c` | demo source |
| `tools/build-initramfs.py` | cross-build hello.ko, install at /lib/modules/ |
| `Makefile` | `test-module` regression target |

~1100 lines total.  Most of it is the ELF parser glue and
the relocation handlers; the loader's orchestration is small
because each pass is straightforward once the data is shaped.

## Status

| Surface | Status |
|---|---|
| K1 — ELF .ko parser + ksymtab + reloc + init | ✅ |
| K1 demo: `hello.ko` prints + returns | ✅ |
| K1 trampolines: out-of-range CALL26 routing | ✅ |
| K1 x86_64 path | ⏳ K2 |
| K2 — kmalloc / wait_queue / work_queue / completion | ⏳ next session |
| K3-K9 | ⏳ |

## What K2 looks like

K2 is the first heavyweight subsystem milestone.  Linux modules
expect a working memory-allocation interface, sleeping
synchronization primitives, and the `current` macro to find
the running task.  The shape:

- **`kmalloc` / `kfree` / `kcalloc` / `krealloc` / `kvmalloc`.**
  Bridged to Kevlar's existing slab/heap.  Honoring `GFP_*`
  flags is partial — `GFP_KERNEL` and `GFP_ATOMIC` are real;
  `GFP_DMA32` etc. defer until anything actually needs them.
- **`wait_queue_head` / `wake_up` / `prepare_to_wait` /
  `finish_wait`.**  Linux's main sleep primitive.  Map to
  Kevlar's existing waitqueue.
- **`work_struct` / `INIT_WORK` / `schedule_work` /
  `flush_work`.**  Bottom-half deferred work.  Bridge to a
  per-cpu kthread.
- **`completion` / `wait_for_completion` / `complete`.**
  Single-shot synchronization.
- **`current` macro.**  Resolves to the per-cpu running task
  pointer.  Linux modules read fields off it directly
  (`current->pid`, `current->mm`) — K2 needs to expose a
  Linux-shape `task_struct` skeleton.
- **`module_init()` / `module_exit()` macros + `.modinfo`
  parsing.**  Module declares its entry/exit via macro;
  loader extracts from `.modinfo`.

K2's demo target: a module that schedules a work_struct that
sleeps on a wait_queue_head until completed by a timer.  No
user-visible output beyond a few `printk`s, but it exercises
every K2 subsystem in concert.

K2 is where the kABI surface starts to feel real.  K1 was the
plumbing.
