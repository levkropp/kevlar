# 280 — kABI K33: Phase 1 — three bugs, one strategic pivot

K32 ended with an honest "we found a crash, we don't know
why."  Pcmanfm SIGABRT'd on a folder double-click.  The trap
PC pointed at instruction bytes that didn't appear anywhere
on the 1 GB Alpine disk.  Two hypotheses: ext2 corruption,
or a userspace wild-jump that Linux would also SIGSEGV.

K33 sets out to diagnose first, fix second.  Phase 1 is the
diagnosis: prove or disprove ext2 as the culprit, and lay
the kABI scaffolding for the strategic move (load Linux's
ext4.ko via the FreeBSD-LinuxKPI playbook) regardless of
which way the bug points.

This post is the K33 Phase 1 write-up.  Three independent
bugs surfaced.  Two got fixed.  One got instrumented and
remains rare-but-real.  The strategic move pivoted on a
build-system blocker, but landed with a clean foundation.

## Bug 1 — gvfs-udisks2 SIGTRAP

K32's reproducer was: launch the desktop, watch
`gvfs-udisks2-volume-monitor` die with signal 5 within a
few seconds of pcmanfm starting.  The crash had no trap PC
in the kernel log, no EL0 unhandled exception, nothing.
The process just died.

Diagnostic instrumentation, in order:

1.  **Signal-sender logging.**  We already had a
    `SIGKILL: from pid=X to pid=Y` warning.  Extend to
    SIGTRAP/SIGSEGV/SIGABRT/SIGILL.  First boot after
    rebuild:

    ```
    SIGTRAP: from pid=71 ("/usr/libexec/gvfs/gvfs-udisks2-volume-monitor")
                       to pid=71 ("/usr/libexec/gvfs/gvfs-udisks2-volume-monitor")
    PID 71 (/usr/libexec/gvfs/gvfs-udisks2-volume-monitor) killed by signal 5
    ```

    `from pid=71 to pid=71` — gvfs is calling
    `tkill(self, SIGTRAP)` on itself.  That narrows the
    search dramatically: no kernel-side BRK handler is
    involved; this is a userspace `raise(SIGTRAP)` from
    within glib's `G_BREAKPOINT()` macro, which fires from
    `g_error()` and fatal `g_critical()`.

2.  **strace-comm prefix match + stderr capture.**  We
    already had `strace-pid=N` for tracing one PID.  Add
    `strace-comm=NAME`.  Linux's `comm` is truncated to
    `TASK_COMM_LEN=16` so `gvfs-udisks2-volume-monitor`
    becomes `gvfs-udisks2-vol`; prefix-match so the user
    can write `strace-comm=gvfs-udisks2` and still hit
    it.  Then in `sys_writev` and `sys_write`, when the
    matched-comm process writes to fd 2 (stderr), dump
    up to 256 bytes on the debug serial.

    Reboot:

    ```
    DBG {"type":"syscall_entry","pid":71,"name":"writev","nr":66,"args":[2,...,2,...]}
    STRACE-STDERR pid=71 (96 bytes):
    (process:71): GLib-GIO-ERROR **: 18:06:29.120: No GSettings schemas
                  are installed on the system
    DBG {"type":"syscall_entry","pid":71,"name":"tkill","nr":130,"args":[71,5,...]}
    ```

    Root cause: `apko build-minirootfs` does not run
    package postinstall scripts.  Adding `gvfs` to the
    apko package list lays down the `.gschema.xml` source
    files but does not generate
    `/usr/share/glib-2.0/schemas/gschemas.compiled`.
    Without it, glib's `g_settings_new()` calls
    `g_error("No GSettings schemas...")` →
    `G_BREAKPOINT()` → `raise(SIGTRAP)`.

The fix is two lines: add `gsettings-desktop-schemas` to
the apko package set, then run `glib-compile-schemas` on
the rootfs after extraction.  The output format is GVariant
— architecture-independent — so the macOS host's
`glib-compile-schemas` (from Homebrew) produces a working
cache for arm64 guests.

```python
# tools/build-alpine-lxde.py
schemas_dir = root / "usr" / "share" / "glib-2.0" / "schemas"
if schemas_dir.is_dir():
    schema_files = list(schemas_dir.glob("*.gschema.xml"))
    if schema_files:
        compiler = shutil.which("glib-compile-schemas")
        subprocess.run([compiler, str(schemas_dir)],
                       capture_output=True, text=True)
```

After: 8/8 LXDE pass clean.  No more gvfs-udisks2 SIGTRAP.

This is also a memorable pattern: **the kernel was right;
the rootfs was wrong**.  No Kevlar code change was needed.
That's good news for the "drop-in Linux" goal — the apko
behavior is the same on every distro that uses apko, so
this fix benefits any future Kevlar rootfs build.

## Bug 2 — PAGE_ZERO_MISS at PREZEROED_POOL

The pcmanfm freeze had a different signature.  When
test-lxde double-clicked a folder, an EL0 unhandled
exception fired:

```
EL0 unhandled exception: ec=0x0 esr=0x2000000 pc=0xa10492618
                          far=0xa0006a6f0 insn=0x41f51660
PID 25 (/usr/bin/tint2) killed by signal 11
```

`insn=0x41f51660` doesn't decode as a valid arm64
instruction.  And right after, an existing instrumentation
fired:

```
PAGE_ZERO_MISS site=PREZEROED_POOL paddr=0x73297000
                first_nz_off=0x618 nonzero_words=32
                kernel_ptr_words=4
    +0x618: 0xffff000041f51660 <<<
```

The **lower 32 bits of the kernel pointer at offset 0x618
of the corrupted page are exactly the trapping insn**.  The
process loaded "instructions" that were really stale kernel
data.  The page came from `PREZEROED_POOL` which is supposed
to hand out zeroed pages — and one of them wasn't zero.

This rules out ext2 corruption.  The page is corrupt
*before* it ever reaches the user.  Phase-1 hypothesis
status:

- ❌ ext2 corruption — disproven; the bytes never came
  from disk.
- ✅ kernel-mm bug — confirmed; some kernel writer is
  poking at a paddr that's currently in PREZEROED_POOL.

That's the smoking gun.  We just have to find the writer.

The instrumentation expansion:

1.  **Port the free-tracking ring buffers from x86 to
    arm64.**  `SINGLE_FREE_RING` and `MULTI_FREE_RING`
    record `(paddr, tsc, caller_rip)` for every recent
    `free_pages()` call so a later PAGE_ZERO_MISS can
    correlate against the most-recent free site.  The x64
    impl uses `rdtsc` and an RBP walk; arm64 uses
    `mrs CNTVCT_EL0` and `ldr [x29, #8]` (frame pointers
    are forced via `-Cforce-frame-pointers=yes` in the
    target spec).

2.  **Periodic pool sweep.**  From `interval_work()`,
    every ~0.5 s, scan every page sitting in
    `PREZEROED_POOL` for non-zero data.  This catches the
    write closer to the wall-clock moment it happens
    rather than waiting for a user to pop the page.

The first version of the sweep racey-fired 30+ times per
boot — false positives from a TOCTOU between snapshot and
scan: the pool would pop a page legitimately, hand it to
a user, the user would write to it, and my scan would see
the user's data and accuse it of corruption.  Fixed by
re-acquiring the pool lock and confirming pool-membership
before flagging.

3.  **Double-free detector.**  At `free_pages()` entry,
    check whether the paddr is already queued in
    `PAGE_CACHE` or `PREZEROED_4K_POOL`.  A second free
    of an already-queued page would corrupt it (the buddy
    `push_free` writes a freelist next-pointer at
    offset 0).  Logs the caller LR + the prior free site
    from `SINGLE_FREE_RING` and silently drops the second
    free rather than letting the buddy corrupt itself.

After all three landed, **0 events across 20 LXDE boots**.
The original PAGE_ZERO_MISS bug is real but rare.  My
inability to reproduce it doesn't mean it's gone — it
means the timing window is narrow.  The instrumentation
will catch it when it next occurs, with the caller LR
identifying the source.

This is the kind of bug that resolves "in a future
session, on a future random-timing build" rather than now.
Mark and move on.

## Bug 3 — openbox occasional SIGSEGV

A null-deref in openbox's signal handler chain.  Showed
up in one of the user's logs; doesn't repro in 20 runs.
Same triage as bug 2: instrumented, deferred.

## The strategic move — load Linux's ext4.ko via kABI

The ambient question for K33 was: now that we know it's
not ext2 corruption, do we still want to replace
`kevlar_ext2` with Linux's `ext4.ko`?

Answer: **yes**, because the strategic value is unchanged
— Kevlar's "drop-in Linux replacement" north-star means
loading Linux .ko binaries natively (the FreeBSD LinuxKPI
playbook).  The 9 modules already loaded via kABI prove
the idea works for trivial drivers (input, DRM helpers,
display).  Ext4 is the test of "does the playbook scale
to non-trivial subsystems?"  If kABI for filesystems
works, network drivers and GPU drivers follow trivially.

We hit one blocker fast: **Ubuntu builds ext4 builtin
(`CONFIG_EXT4_FS=y`)**, so it's not in the modules deb
that `tools/fetch-ubuntu-linux-modules.sh` already
downloads.  We have the kernel source at
`build/linux-src.vanilla-v7.0/`, and `aarch64-linux-musl-gcc`
on PATH, but `make modules_prepare` fails on
`scripts/mod/file2alias.c`:

```
error: member reference base type 'typeof (((struct
       tee_client_device_id *)0)->uuid)' (aka
       'unsigned char[16]') is not a structure or union
```

Apple Clang has stricter typeof+member-access semantics
than GCC.  Cross-building Linux kernel modules from macOS
is a Docker / GNU GCC environment problem, separate from
this milestone.

Tactical pivot: **scaffold the kABI surface anyway**.
The block layer + page-cache + fs-registry + jbd2 surface
is the same regardless of which filesystem .ko we load;
once the scaffolding is in place, the actual module is a
drop-in test.

Four new files, all stubs returning null/no-op:

- **`kernel/kabi/block.rs`** (~30 fns): `bio_*`, `bdev_*`,
  `__bread/__getblk`, `sb_bread/sb_getblk`,
  `super_setup_bdi`, `errno_to_blk_status`.
- **`kernel/kabi/filemap.rs`** (~20 fns): folio
  alloc/lookup/lifecycle, `filemap_read`,
  `truncate_inode_pages_final`,
  `invalidate_mapping_pages`, dcache flush.
- **`kernel/kabi/fs_register.rs`**: `register_filesystem`
  + `unregister_filesystem` are *real* — they record the
  module's `*file_system_type` into a SpinLock'd
  `ArrayVec<_, 16>`.  `lookup_fstype()` exposed for the
  Phase 3 mount-syscall route.
- **`kernel/kabi/jbd2_stubs.rs`**: jbd2_journal_*
  returning a non-null fake-handle so ext4's
  `IS_ERR(handle)` check passes during RO mount with
  `noload`.

Counting:

```
$ grep -rh '^ksym!(' kernel/kabi/*.rs | wc -l
381
```

333 → 381 exported kABI symbols.  arm64 + x64 build clean.
LXDE 8/8 still passes (no behaviour change since none of
the new functions are called yet).

Erofs.ko (already shipped as a module in Ubuntu's deb) is
the available proof-of-concept.  Its undefined-symbol set
is 271 entries; 30 are now resolved by the new
scaffolding, the remaining ~241 split between existing
kABI modules and stubs we'll add iteratively.

## What ships in K33 Phase 1

- Three bugs investigated.  One fixed at the rootfs-build
  layer, two instrumented for future capture.
- Three new diagnostics, kept on by default (cheap):
    - SIGTRAP/SIGSEGV/SIGABRT/SIGILL sender logging.
    - strace-comm prefix-match + stderr fd-2 capture
      under traced comm.
    - PREZEROED_POOL periodic sweep + DOUBLE_FREE
      detector + arm64 free-tracking ring port.
- Four new kABI files, 612 lines, 48 new exports.  The
  filesystem-loading playbook has its skeleton; the
  scaffolding is shippable on its own.

## Status

| Milestone | Status |
|---|---|
| K1-K29: kABI arc | ✅ |
| K30: graphical Alpine LXDE | ✅ |
| K31: trackpad + 12s boot + quiet console | ✅ |
| K32: itest framework + EL0 hardening + freeze diagnosed | ✅ |
| K33 Phase 1: gvfs SIGTRAP fixed; mm bugs instrumented; kABI fs scaffolded | ✅ |
| K33 Phase 2: implement bodies (ext4 / erofs load attempt) | ⏳ |
| K33 Phase 3: route mount(2) through kABI for ext4/ext2 | ⏳ |

Phase 2 is the next session: pick the `.ko` file, attempt
to load, fill in the stubs the loader actually needs.
The plan estimates 1-2 weeks; the work decomposes cleanly
into many tight commits.
