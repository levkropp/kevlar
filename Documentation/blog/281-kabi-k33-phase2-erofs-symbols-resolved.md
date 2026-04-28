# 281 — kABI K33 Phase 2: erofs.ko, 204 → 0 unresolved

K33 Phase 1 was diagnosis + scaffolding: prove the pcmanfm
freeze isn't ext2 corruption (it's not), fix the gvfs SIGTRAP
(missing GSettings schemas), and lay the file structure for
loading a Linux filesystem .ko via kABI.  Phase 1 ended with
`kernel/kabi/{block,filemap,fs_register,jbd2_stubs}.rs` —
empty shells, stubs returning null/no-op.  48 new kABI
exports.  The actual `.ko` wasn't yet loaded.

K33 Phase 2 fills in the surface.  This post is the
write-up of the bring-up: from "loader bails on the first
unresolved symbol" to "all 204 of erofs.ko's external
symbols resolve, init_module dispatches, returns
gracefully."  No filesystem mount yet — Phase 3 — but the
playbook is proven.

## Why erofs and not ext4

The K33 plan said ext4.  ext4 is the *strategic* target:
the most-used Linux filesystem, the de-facto baseline for
"can Kevlar mount the same disk Linux mounts."  Phase 2
opens with trying to source `ext4.ko` and hits an
immediate wall:

  * **Ubuntu builds ext4 builtin** (`CONFIG_EXT4_FS=y`).
    The `linux-modules-7.0.0-14-generic.deb` we already
    fetch via `tools/fetch-ubuntu-linux-modules.sh` has
    every other filesystem as a `.ko` (xfs, btrfs, 9p,
    erofs, ubifs, jfs, hpfs, …) but not ext4.

  * **Cross-building from macOS fails.**  We have the
    full kernel source at `build/linux-src.vanilla-v7.0/`
    and `aarch64-linux-musl-gcc` on PATH.
    `make modules_prepare` falls over in
    `scripts/mod/file2alias.c`:

    ```
    error: member reference base type 'typeof (((struct
           tee_client_device_id *)0)->uuid)' (aka
           'unsigned char[16]') is not a structure or union
    ```

    Apple Clang's typeof+member-access is stricter than
    GCC's.  Cross-building Linux kernel modules from
    macOS needs Docker / GNU GCC — separate task.

So Phase 2 pivots: **erofs.ko** (Enhanced Read-Only File
System) is in the deb, exercises the same kABI surface as
ext4 minus jbd2 journaling, and proves the playbook.
Once it works, ext4 follows immediately as soon as the
.ko is built (task #99).

## The loader's bail-on-first problem

Initial attempt:

```
kabi: loading /lib/modules/erofs.ko (Ubuntu 26.04, K33 Phase 2)
kabi: loaded /lib/modules/erofs.ko (299737 bytes, 60 sections, 1045 symbols)
kabi: undefined external symbol 'validate_usercopy_range' — not in kernel exports table
kabi: erofs load_module failed: ENOENT
```

The loader bailed on the very first unresolved symbol.
That's *correct* behaviour — a module with unresolved
symbols can't run — but it makes incremental development
glacial.  For each unresolved symbol you (1) find it in
Linux source to figure out the signature, (2) write a
stub, (3) `cargo build` (~30 s on this hardware), (4)
boot QEMU (~10 s), (5) hit the next missing symbol, repeat.

For 200+ symbols that's a multi-day task before any forward
progress.  The fix is one well-placed pre-pass:

```rust
// kernel/kabi/loader.rs, before relocation
let mut missing: Vec<&str> = Vec::new();
for sym in obj.symtab.iter() {
    if sym.st_shndx == symbols::SHN_UNDEF {
        let name = obj.sym_name(sym);
        if !name.is_empty()
            && exports::lookup(name).is_none()
            && !missing.iter().any(|n| *n == name)
        {
            missing.push(name);
        }
    }
}
if !missing.is_empty() {
    log::warn!("kabi: {} undefined external symbol(s) for {}:",
               missing.len(), path);
    for name in &missing {
        log::warn!("    UNDEF: {}", name);
    }
}
```

A single boot now spits the entire missing-symbol roster:

```
kabi: 204 undefined external symbol(s) for /lib/modules/erofs.ko:
    UNDEF: filp_open
    UNDEF: dynamic_preempt_schedule_notrace
    UNDEF: iomap_readahead
    ...
```

That turns "200 round-trips" into "categorize and batch."

## Categorizing 204 symbols

A `grep -E UNDEF /tmp/k33-erofs.log | sed ... | sort -u`
gives the full list.  Eyeballing categories:

| Category | Count | Strategy |
|---|---|---|
| libc-ish (memcmp, strlen, strncmp…) | 15 | Real impls, ~10 lines each |
| BPF/perf/tracepoints | 16 | Pure no-ops |
| VFS (dentry/inode/super) | 25 | Null returns; fs init checks them |
| iomap | 10 | -ENOSYS; erofs falls back to plain reads |
| DAX | 4 | -ENOSYS; not on QEMU virtio-blk |
| Crypto/decompression | 21 | -1/null; erofs uncompressed path |
| Sync (mutex/down/up/RCU) | 18 | No-ops; single-threaded init context |
| xarray / IDR | 10 | Always-empty stubs |
| Workqueue / kthread / shrinker | 9 | No-ops |
| Page allocator (Linux-named) | 10 | Null returns for now |
| Slab (kmem_cache_*) | 4 | Null returns (the wall — see below) |
| CPU mask / hotplug | 8 | Static mask with bit 0 set |
| FS params, kobject, scatterlist | 7 | -EINVAL/null |
| Misc | 47 | Per-symbol triage |
| **Total** | **204** | |

I committed the work in three batches:

1.  **Pass 1** (`kernel/kabi/mem.rs` + `tracepoints.rs`):
    libc + tracepoint surface.  204 → 173.

2.  **Pass 2** (`kernel/kabi/fs_stubs.rs`, NEW): 173 entries
    in one bulk file, organised by sub-section.  The shape
    is repetitive enough to copy-paste fast — each entry is

    ```rust
    #[unsafe(no_mangle)]
    pub extern "C" fn iomap_dio_rw(_iocb: *mut c_void,
                                   _iter: *mut c_void,
                                   _ops: *const c_void,
                                   _dops: *const c_void,
                                   _flags: u32,
                                   _private: *mut c_void,
                                   _done_before: usize) -> isize { -38 }

    ksym!(iomap_dio_rw);
    ```

    173 → 1.

3.  **Pass 3**: the last holdout was `vsnprintf`, which I'd
    accidentally `extern "C"`-imported instead of stubbing.
    Drop the import, add a no-op:

    ```rust
    #[unsafe(no_mangle)]
    pub extern "C" fn vsnprintf(buf: *mut u8, size: usize,
                                _fmt: *const u8,
                                _args: *mut c_void) -> c_int {
        if !buf.is_null() && size > 0 {
            unsafe { *buf = 0 };
        }
        0
    }
    ```

    1 → 0.

## The successful load

```
kabi: loading /lib/modules/erofs.ko (Ubuntu 26.04, K33 Phase 2)
kabi: loaded /lib/modules/erofs.ko (299737 bytes, 60 sections, 1045 symbols)
kabi: /lib/modules/erofs.ko license=Some("GPL")
                              author=Some("Gao Xiang, Chao Yu, Miao Xie...")
                              desc=Some("Enhanced ROM File System")
kabi: image layout: 149968 bytes (37 pages) at 0xffff00007cdc0000
kabi: all external symbols resolved for /lib/modules/erofs.ko
kabi: applied 3533 relocations (143 trampoline(s))
kabi: erofs init_module returned -12
kabi: registered filesystems = 0
```

`-12` is `-ENOMEM`.  Erofs's init walks down through:

  1.  `kmem_cache_create()` — succeeds (existing kABI).
  2.  `kmem_cache_alloc_lru_noprof()` for an inode-cache
      slab — **returns null from our v1 stub**.
  3.  Erofs aborts init, returns -ENOMEM.
  4.  Loader unwinds, `init_module returned -12`.

This is the right shape of failure — it's a known stub, not
a kernel-side crash, not a wild memory access, not a missing
symbol.  The next round of work (Phase 2b, task #100) is to
wire `kmem_cache_alloc_lru_noprof` and `kobject_init_and_add`
to real impls so init progresses past the slab call into
`register_filesystem()`.

## The default-boot gate

Loading erofs.ko while it returns -ENOMEM has a
side-effect: even though init returned cleanly, partial
state was left in some global table that wedged later
`virtio_input.ko` probe.  Default boot now hangs around the
input-register step.

The fix isn't to figure out what state — that'd be a bug
hunt for a feature that's still half-built.  The fix is to
gate the load attempt:

```rust
#[cfg(target_arch = "aarch64")]
if bootinfo.raw_cmdline.as_str().contains("kabi-load-erofs=1") {
    info!("kabi: loading /lib/modules/erofs.ko ...");
    match kabi::load_module("/lib/modules/erofs.ko", "init_module") {
        Ok(m) => match m.call_init() {
            Some(rc) => {
                info!("kabi: erofs init_module returned {}", rc);
                info!("kabi: registered filesystems = {}",
                      kabi::fs_register::registered_count());
            }
            ...
```

Default boot: 8/8 LXDE pass, no behaviour change.  Debug
runs: `--append-cmdline "kabi-load-erofs=1"` and the load
fires.  This is the same idiom Linux uses for experimental
features (`module_param` with sane defaults).

## What's actually shippable

What's gone from "wishlist" to "real code" in this Phase 2
session:

  * Loader pre-pass — every future module bring-up sees
    the full undef-symbol roster in one boot.

  * 204 stubs across 3 files (mem.rs, tracepoints.rs,
    fs_stubs.rs).  Every one is one of: a real impl
    delegating to platform code; a no-op; a sentinel
    return.  None pretend to do work they can't.

  * kABI export count 333 → ~580 (~73% growth).  The
    erofs surface is now the single largest chunk of
    kABI by symbol count, ahead of the 9 previously
    loaded modules combined.

  * Initramfs ships `/lib/modules/erofs.ko` (300 KB).

  * `kabi-load-erofs=1` cmdline gate keeps the default
    safe while debug runs are one append-cmdline away.

What's still ahead (Phase 2b, then Phase 3):

  * Real `kmem_cache_alloc_*` so init progresses past
    the inode-cache step into `register_filesystem()`.
    The kABI `slab` module already exists; just wire
    the names through.

  * Real `kobject_init_and_add` (or a stub that doesn't
    poison later virtio_input probe).

  * `mount(2)` syscall routing: when fstype is
    `ext4`/`erofs`/anything in our kABI fs registry,
    dispatch through `kabi::fs_register::lookup_fstype`
    and the (yet-to-exist) `KabiFileSystem` adapter
    instead of the homegrown `kevlar_ext2`.

## Status

| Milestone | Status |
|---|---|
| K1-K29: kABI arc | ✅ |
| K30: graphical Alpine LXDE | ✅ |
| K31: trackpad + 12s boot | ✅ |
| K32: itest + freeze diagnosed | ✅ |
| K33 Phase 1: gvfs fix; mm bug instrumented; fs scaffolding | ✅ |
| K33 Phase 2: erofs.ko 204 → 0 unresolved | ✅ |
| K33 Phase 2b: kmem_cache / kobject real impls | ⏳ |
| K33 Phase 3: route mount(2) through kABI | ⏳ |
| K34+: ext4.ko build (Docker / GNU GCC env) | ⏳ |

The interesting moment is when erofs's init returns 0 (not
-ENOMEM) and the kABI fs registry has 1 entry.  That's the
first time Kevlar's "drop-in Linux replacement" tagline
extends from "load Linux's GPU drivers" to "load Linux's
filesystems."  And the playbook for that transitive jump
is now visible end-to-end.
