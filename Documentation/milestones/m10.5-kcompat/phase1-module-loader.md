# M10.5 Phase 1: Module Loader

**Goal:** Parse, link, and execute Linux 6.18 `.ko` kernel modules.
Success criterion: `insmod hello.ko` prints "Hello, Kevlar!" and `rmmod hello`
unloads cleanly.

---

## What is a .ko file?

A `.ko` is an ELF `ET_REL` (relocatable object), not an executable. It
contains:

- **`.text` / `.data` / `.bss`**: Module code and data
- **Undefined symbols**: Functions the module calls into the kernel
  (e.g., `printk`, `pci_enable_device`)
- **Defined symbols**: Functions the kernel can call into the module
  (`init_module`, `cleanup_module`, or the `module_init`/`module_exit` aliases)
- **Relocations**: `R_X86_64_PC32`, `R_X86_64_64`, etc. — fixups applied at
  load time once the module is placed in memory
- **Special sections**:
  - `.modinfo`: NUL-separated `key=value` strings (`vermagic`, `author`, etc.)
  - `__versions`: Table of `{ crc: u32, name: [u8; 64] }` for each imported symbol
  - `__ksymtab` / `__ksymtab_strings`: Symbols the module exports to other modules
  - `.gnu.linkonce.this_module`: `struct module` metadata (name, init, exit)

---

## vermagic check

Every module embeds a `vermagic` string in `.modinfo`, e.g.:

```
vermagic=6.18.0-arch1-1 SMP preempt mod_unload
```

The running kernel must present the same vermagic or the load is rejected.
Kevlar's kcompat layer presents:

```
vermagic=6.18.0 SMP preempt mod_unload
```

(Exact 6.18 patchlevel string, without distro suffix — modules compiled
against vanilla 6.18 headers will match.)

---

## Symbol CRC table

The `__versions` section contains one `{ u32 crc, char name[64] }` entry
per imported symbol. The kernel checks each CRC against its own export table.
A mismatch aborts the load.

kcompat maintains `kcompat/symbols_6_18.rs`: a static array of
`(name: &str, crc: u32)` generated from Linux 6.18's `Module.symvers`.
This file is generated once and committed — it does not change within the
6.18 LTS series.

```rust
// Auto-generated from Linux 6.18 Module.symvers
pub static EXPORTED_SYMBOLS: &[(&str, u32)] = &[
    ("printk",              0x3a4b5c6d),
    ("kmalloc",             0x1234abcd),
    ("pci_enable_device",   0x9f8e7d6c),
    // ... ~30,000 entries for all exported kernel symbols
    // In practice, Tier 1 drivers use ~200-500 of these
];
```

On module load, kcompat walks `__versions` and looks up each name in the
table. If all CRCs match, the load proceeds.

---

## Loading sequence

```
insmod /path/to/module.ko
   │
   ├── read ELF, parse sections
   ├── check vermagic
   ├── verify symbol CRCs (__versions vs kcompat table)
   ├── allocate executable memory (alloc_pages + mark exec)
   ├── copy .text, .data, .bss to allocated region
   ├── apply relocations (R_X86_64_64, R_X86_64_PC32, R_X86_64_PLT32, ...)
   ├── resolve undefined symbols → kcompat function pointers
   ├── call module->init() (= init_module or module_init alias)
   └── add to MODULES list (/proc/modules)

rmmod module_name
   ├── find in MODULES list
   ├── check refcount == 0
   ├── call module->exit() (= cleanup_module or module_exit alias)
   ├── free allocated memory
   └── remove from MODULES list
```

---

## Relocation types (x86_64)

The most common relocations in kernel modules:

| Type | Formula | When |
|------|---------|------|
| `R_X86_64_64` | `S + A` | Absolute 64-bit address |
| `R_X86_64_PC32` | `S + A - P` | PC-relative 32-bit (call/jmp) |
| `R_X86_64_PLT32` | `L + A - P` | PLT-relative (same as PC32 in kernel) |
| `R_X86_64_32S` | `S + A` | Sign-extended 32-bit |

Where S = symbol value, A = addend, P = relocation location, L = PLT entry.

The kernel module loader doesn't use a PLT — all calls are direct. PLT32
is treated identically to PC32 for in-kernel use.

---

## /proc/modules

`cat /proc/modules` should list loaded modules. Format:

```
hello 16384 0 - Live 0xffffffffc0000000 (O)
nvme 98304 1 nvme_core,Live 0xffffffffc0012000
```

Fields: name, size, refcount, dependents, state, base_addr, taint flags.

---

## Syscall interface

Linux exposes module loading via:
- `init_module(image, len, params)` — load from memory buffer
- `finit_module(fd, params, flags)` — load from fd (more common)
- `delete_module(name, flags)` — unload

`insmod` uses `init_module`/`finit_module`. We need both syscalls.
`params` is a NUL-separated `key=value` string passed to the module.

---

## Hello-world test module

```c
// hello.c — compiled against Linux 6.18 headers
#include <linux/module.h>
#include <linux/init.h>
#include <linux/printk.h>

MODULE_LICENSE("GPL");
MODULE_AUTHOR("Kevlar kcompat test");
MODULE_DESCRIPTION("kcompat smoke test");

static int __init hello_init(void) {
    pr_info("Hello, Kevlar!\n");
    return 0;
}

static void __exit hello_exit(void) {
    pr_info("Goodbye, Kevlar!\n");
}

module_init(hello_init);
module_exit(hello_exit);
```

Compile: `make -C /path/to/linux-6.18-headers M=$(pwd) modules`

This module only uses `printk` (via `pr_info`). Verifying it loads and
unloads correctly validates the entire phase 1 infrastructure before
tackling real drivers.

---

## Implementation plan

1. **`kernel/kcompat/mod.rs`**: crate skeleton, `insmod`/`rmmod`/`finit_module` syscall handlers
2. **`kernel/kcompat/elf.rs`**: ELF ET_REL parser (sections, symbols, relocations)
3. **`kernel/kcompat/relocate.rs`**: x86_64 relocation engine
4. **`kernel/kcompat/symbols.rs`**: symbol lookup table + CRC verification
5. **`kernel/kcompat/symbols_6_18.rs`**: auto-generated 6.18 CRC table
6. **`kernel/kcompat/proc_modules.rs`**: `/proc/modules` pseudo-file
7. **`kernel/kcompat/printk.rs`**: `printk` implementation (first real kcompat symbol)

---

## Files to create/modify

- `kernel/kcompat/` — new crate
- `kernel/Cargo.toml` — add kcompat dependency
- `kernel/syscall/` — wire `init_module`, `finit_module`, `delete_module`
- `kernel/fs/proc/` — add `modules` file
- `Makefile` — `make kcompat-hello` to build test module
