# 300 — Phases 8+9 (ext4 arc): mbcache → jbd2 → ext4 load chain links

The seven-phase erofs arc closed with `cat /mnt/erofs/hello.txt`
through libc.  The next arc replays it for **ext4** — Ubuntu's
default rootfs filesystem, with all the journaling + extent +
extended-attribute machinery that erofs's read-only design lets us
skip.

Phase 8 + 9 ships the load-chain plumbing.  Three modules now load
in sequence with `kabi-load-ext4=1`:

```
kabi: [ext4-arc] loading /lib/modules/mbcache.ko
kabi: registered 10 runtime exports from __ksymtab (120 bytes)
kabi: mbcache init_module returned 0
kabi: [ext4-arc] loading /lib/modules/jbd2.ko
kabi: registered 59 runtime exports from __ksymtab (708 bytes)
kabi: jbd2 init_module returned 0
kabi: [ext4-arc] loading /lib/modules/ext4.ko
kabi: all external symbols resolved for /lib/modules/ext4.ko
kabi: ext4 init_module returned -12
```

ext4's `init_module` returns `-ENOMEM` because some allocation
stub returns NULL where it shouldn't.  Phase 10's job is to trace
that to its specific caller.  The architecturally hard piece —
**ext4.ko's 514 undefined symbols all resolving** — is done.

## Phase 8: inter-module symbol export

The erofs arc never needed this.  erofs.ko has no dependencies on
other `.ko`s — it talks to kernel exports and that's it.  ext4.ko
references **53 jbd2 symbols** + **10 mbcache symbols**.  Linux's
real loader walks every loaded module's `__ksymtab` to resolve
references; ours only checked the kernel's static ksym! table.

### `kernel_symbol` layout

Each entry in `__ksymtab` is 12 bytes (verified against jbd2.ko's
`.rela__ksymtab` section, 177 entries / 3 relocs per entry = 59
exported symbols):

```
struct kernel_symbol {
    s32 value_offset;     // PREL32 → function VA
    s32 name_offset;      // PREL32 → __kstrtab_<name>
    s32 namespace_offset; // PREL32 → namespace string (0 for ROOT)
};
```

After the loader's existing relocation pass runs, each `s32` holds
the correct PC-relative offset.  Computing the resolved values:

```
function_va = (entry_va + 0) + value_offset
name_va     = (entry_va + 4) + name_offset
```

### The runtime table

`kernel/kabi/exports.rs`:

```rust
pub mod runtime {
    pub static RUNTIME_EXPORTS: SpinLock<Vec<RuntimeExport>> = ...;

    pub fn lookup(name: &str) -> Option<usize> { ... }
    pub fn register(name: &str, addr: usize) { ... }
}

pub fn lookup(name: &str) -> Option<usize> {
    if let Some(s) = static_lookup(name) { return Some(s); }
    runtime::lookup(name)  // ← Phase 8 fallback
}
```

### Loader hook

After relocations apply, scan `__ksymtab` and `__ksymtab_gpl` for
each loaded module:

```rust
for ksymtab_name in &["__ksymtab", "__ksymtab_gpl"] {
    for (i, sh) in obj.sections.iter().enumerate() {
        if obj.section_name(sh) != *ksymtab_name { continue; }
        let sec_va = section_va_map[i].unwrap();
        register_ksymtab_entries(sec_va, sh.sh_size, ksymtab_name);
    }
}
```

`register_ksymtab_entries` walks 12-byte chunks, decodes the
PREL32 offsets, reads the NUL-terminated name string, and
`exports::runtime::register(name, function_va)`.

Side effect we didn't anticipate: our **existing** loaded modules
all have `__ksymtab` sections too.  Boot now logs:

```
kabi: registered 10 runtime exports from __ksymtab (k3.ko)
kabi: registered 7 runtime exports from __ksymtab (k4.ko)
kabi: registered 12 runtime exports from __ksymtab_gpl (k5.ko)
...
40 runtime exports total at boot (from K-demo + Ubuntu modules)
```

Free real estate.

## Phase 9: stubbing the rest

ext4.ko links against 514 undefined symbols.  Of those:

| Source | Count |
|---|---|
| Already in kernel `ksym!()` table | ~250 |
| Now provided by jbd2.ko's runtime exports | ~50 |
| Now provided by mbcache.ko's runtime exports | ~10 |
| Still missing → stubbed in this commit | **261** |

I split the 261 into two files:

  * `kernel/kabi/ext4_arc_stubs.rs` (~75 hand-written): wait queue
    primitives, scheduler, locks, timers, buffer-head shims, page
    allocators, percpu counters, procfs, seq_file, filemap
    writeback paths, crc32_be, errseq.  All no-ops or null returns
    appropriate for the RO-with-`noload` mount path.

  * `kernel/kabi/ext4_arc_bulk_stubs.rs` (auto-generated, 261 stubs):
    one Python script consumes the boot log's `UNDEF: <name>` lines
    and emits a generic
    `pub extern "C" fn name(_,_,_,_,_,_) -> *mut c_void { null }`
    for each.  Most are mount-path functions ext4's init_module
    never calls.  The ABI sloppy-args trick works because ARM64
    AAPCS tolerates extra register args (caller writes x0-x7,
    callee ignores).

The point of bulk-stubbing isn't to make the calls work — it's to
make the **module link** so the actually-load-time functions
(`__kmem_cache_create_args`, `register_filesystem`, etc.) can run.
Phase 10 will replace the specific stubs that ext4's mount path
actually depends on.

## Why Phase 8 was the unlock

erofs.ko had this property: **every undefined symbol it referenced
was either a kernel built-in or in our existing kABI shim**.
Adding stubs was linear with the symbol list.

ext4.ko has 53 undefined symbols that are exported by jbd2.ko —
they're inside another `.ko` we'd need to load first.  Without
inter-module export, those 53 would each need a hand-written stub
that reimplements jbd2's actual logic.  Some of those (journal
commit, log block allocation) are non-trivial.

Phase 8 means we don't write any of those stubs.  We compile + load
jbd2.ko, its real implementations populate the runtime table, and
ext4.ko at link time finds them.  That's the entire point of
EXPORT_SYMBOL in real Linux, faithfully replicated.

## What's next

| Phase | Goal | Status |
|---|---|---|
| 8 — inter-module exports | runtime ksymtab table + lookup | ✅ |
| 9 — load chain links | mbcache + jbd2 + ext4 init_module dispatch | ✅ |
| 10 — make init_module return 0 | trace the -ENOMEM caller, replace null stub | ⏳ |
| 11 — block_device synth | `bdev_file_open_by_path` wraps virtio_blk | ⏳ |
| 12 — fc_fill_super for ext4 | mount succeeds in-kernel | ⏳ |
| 13 — userspace test | `mount -t ext4 /dev/vda /mnt/ext4` | ⏳ |

Default boot 8/8 LXDE clean; Phase 7 erofs test still 8/8 PASS
throughout.  The compat layer's incremental design — every blocker
is one disasm + one stub away — keeps paying off.
