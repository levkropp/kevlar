# 304 — Phase 12 v9 (ext4 arc): `EXT4 PROBE: fill_super returned 0 PASS`

After eight failed sub-versions, ext4.ko's `fill_super` finally
returns 0 against a fresh `mkfs.ext4` fixture mounted via the kABI
layer.  The fix that finally landed it is two lines of Rust.  The
*reason* it took eight sub-versions to find those two lines is the
real story.

End-state log:

```
kabi: get_tree_bdev: dispatching fill_super(sb, fc)
kabi: submit_bh READ blocknr=1 size=1024  → ok  (ext4 superblock)
kabi: submit_bh READ blocknr=0..36       → ok  (block group tables)
kabi: d_make_root: dentry=0xffff...0410 inode=0xffff...c120
kabi: get_tree_bdev: fill_super returned 0
EXT4 PROBE: fill_super returned 0 PASS
```

Phase 7 erofs test still 8/8 PASS.  Default boot 8/8 LXDE clean.

## The trail of bodies

Phase 12 v6 shipped real `__lock_buffer` / `unlock_buffer` /
`end_buffer_read_sync` semantics.  Stuck at -EIO post-d_make_root.

Phase 12 v7 tried wholesale `pr_err` instrumentation in
`__ext4_fill_super`.  The patched .ko panicked at `far=0x6c4`
*before* any pr_err could fire.  Reverted.

Phase 12 v8 swapped pr_err for a fixed-shape
`kabi_breadcrumb(line, target_id, err)` ksym + ring buffer +
flexible patcher.  Reasoned that pr_err's variadic format string
must be the layout-sensitive trigger.  But the breadcrumb-instrumented
.ko **also** panicked at `far=0x6c4`, head=0 on dump.  Conclusion at
the time: any patch to `__ext4_fill_super` triggers the fault, ergo
either a kABI loader bug (relocation handling) or ext4-internal
code sensitivity to layout.

That conclusion was wrong on every count.

## Two stacked bugs (and one process bug)

### Bug 1: `if (err) goto X;` is single-statement

The patcher injected `kabi_breadcrumb(...)` *before* the goto:

```c
err = ext4_load_super(sb, &sb_block, silent);
if (err)
    kabi_breadcrumb(5348, 729, err);   /* INSERTED */
    goto out_fail;
```

Which is exactly:

```c
err = ext4_load_super(sb, &sb_block, silent);
if (err)
    kabi_breadcrumb(5348, 729, err);
goto out_fail;                          /* unconditional now */
```

Single-statement-if: only the next statement is guarded.  Adding a
sibling drops the goto out of the conditional.  So **every patched
build's first goto fired unconditionally** — at function entry,
with `err` = 0 and the rest of the cleanup state un-set.  Hitting
`out_fail:` with sb->s_bdev = NULL meant `invalidate_bdev(NULL)`,
which is the `far=0x6c4` deref (some buried offset in struct
super_block).

The fix is a 30-character do/while wrapper:

```python
instr = (
    f'{indent}do {{ kabi_breadcrumb({i}, {tid}, {err_var}); '
    f'goto {target}; }} while (0); {SENTINEL}\n'
)
```

`do { ... } while (0)` is a single statement.  The conditional is
preserved.  Standard idiom for multi-statement macros.

### Bug 2 (the real one): `sb->s_flags` was never set

Once breadcrumbs actually fired, the trail led straight to it.
Label-mode instrumentation showed control reaching `failed_mount4a`
with err=-5.  Goto-mode pinpointed line 5614:

```c
err = ext4_setup_super(sb, es, sb_rdonly(sb));
if (err == -EROFS) {
    sb->s_flags |= SB_RDONLY;
} else if (err)
    goto failed_mount4a;       /* ← line 5614, err = -5 */
```

`ext4_setup_super` for read-only mode is supposed to be a no-op:

```c
if (le32_to_cpu(es->s_rev_level) > EXT4_MAX_SUPP_REV) { ... }
if (read_only) goto done;       /* RO short-circuit */
... /* write path */
done:
    return err;                 /* err = 0 */
```

But `read_only` came from `sb_rdonly(sb)`, which reads
`sb->s_flags & SB_RDONLY`.  Our zero-initialized synth sb had
`s_flags = 0`.  So `sb_rdonly()` returned false, ext4 ran the
write path, `ext4_commit_super` wrote to the (read-only) bdev,
got -EIO.

The fix: propagate `fc->sb_flags` to `sb->s_flags` in
`get_tree_bdev_synth`.  One unsafe write at offset +80.

```rust
let fc_sb_flags = unsafe {
    *(fc.cast::<u8>().add(fl::FC_SB_FLAGS_OFF) as *const u32)
} as u64;
unsafe {
    *(s.add(fl::SB_S_FLAGS_OFF) as *mut u64) = fc_sb_flags;
}
```

### Bug 3 (process): the initramfs was stale

This is the one that turned a half-day into two days.  After
rebuilding `kabi-modules` with instrumentation, breadcrumbs **never
fired** — head=0 on dump even with 36 patched goto sites and the
.ko clearly containing 35 `bl kabi_breadcrumb` calls.  We chased
this for hours: PAuth?  SCS overflow?  Loader reloc bug?  Trampoline
broken?

The actual cause: `make ARCH=arm64 build` sees that the kernel ELF
is up-to-date and skips invoking `tools/build-initramfs.py`.  Which
means the patched `ext4.ko` in `build/kabi-modules/` never gets
copied into `build/initramfs-rootfs/lib/modules/`, and the boot
runs **stock** ext4.ko while we believe we've loaded the patched
one.

Diagnostic that finally cracked it:

```
$ cmp build/kabi-modules/ext4.ko \
      build/initramfs-rootfs/lib/modules/ext4.ko
... differ: char 41, line 1
```

849336 bytes vs 850064 bytes — completely different files.  Once
manually re-running `python3 tools/build-initramfs.py` regenerated
the rootfs, breadcrumbs fired immediately and the path to the
real bug took 30 minutes.

## What the breadcrumbs actually showed

Once everything worked, label-mode (instrument every label-entry
body) gave the cleanest trace:

```
kabi-bc: line=5750 target_id=290 err=-5    ← failed_mount4a (entered here)
kabi-bc: line=5753 target_id=166 err=-5    ← failed_mount4 (fall-through)
EXT4-fs (): mount failed
kabi-bc: line=5757 target_id=81  err=-5    ← failed_mount_wq (fall-through)
kabi-bc: line=5767 target_id=199 err=-5    ← failed_mount3a (fall-through)
kabi-bc: line=5769 target_id=656 err=-5    ← failed_mount3 (fall-through)
kabi-bc: line=5775 target_id=801 err=-5    ← failed_mount (fall-through)
kabi-bc: line=5790 target_id=330 err=-5    ← out_fail (cleanup)
kabi-bc: line=5840 target_id=933 err=-5    ← free_sbi (wrapper cleanup)
```

Reading bottom-to-top: ext4 entered the cleanup chain at
`failed_mount4a`, fell through every label below it, printed
"mount failed" at the failed_mount4 body, finished cleanup, and
returned -EIO.

Then goto-mode (instrument every `goto X;` site) pinpointed which
goto fired:

```
kabi-bc: line=5614 target_id=867 err=-5    ← THE failing goto
```

Line 5614 is the post-`ext4_setup_super` error branch.  Two source
lines later, the bug was visible.

## Patcher upgrades (`tools/docker-kabi-modules/instrument-ext4.py`)

To get to that trace took several patcher fixes:

  * **`do/while` wrapper** (Bug 1 above).  Mandatory for goto-site
    injection.
  * **Inject before extern decl, not after.**  `ensure_extern_decl`
    prepends 4 lines after #include block; running it first shifted
    every `--lines L` target by 4 lines.  Trivial reorder.
  * **Broaden goto regex** from a hand-listed prefix set
    (`failed_mount\w*|out_\w*|err\w*`) to any `goto <label>;`.
    Required to catch the wrapper's `goto free_sbi`.
  * **Auto-detect error variable** (`err` vs `ret` vs `rc`) by
    scanning each function's first 50 lines for an `int X;`
    declaration.  Without this, instrumenting both
    `__ext4_fill_super` (uses `err`) and `ext4_fill_super` wrapper
    (uses `ret`) hit a build error.
  * **`--mode labels`**: inject at label-entry bodies instead of
    goto sites.  Survives compiler inlining/reordering — labels are
    addressable boundaries the optimizer can't easily merge away.

The patcher now has four modes:

| Mode | Where it injects | Use when |
|---|---|---|
| `all` / `functions` | every `goto X;` in the named function(s) | full coverage |
| `lines` | specific source line numbers | bisecting a known suspect |
| `bisect` | half of a function range | binary-search a function |
| `labels` | every label-entry body | inliner-resistant |

## What this unblocks

`kabi_mount_filesystem("ext4", Some("/dev/vda"), MS_RDONLY, ...)`
now returns `Ok(KabiFileSystem)` against a fresh ext4 image.  The
remaining work for an end-to-end userspace `mount -t ext4` test
mirrors what Phase 7 did for erofs:

  * Add `testing/test-kabi-mount-ext4.c` — a C program that calls
    `mount(2)`, `opendir`, `readdir`, `open`, `read`.
  * Wire it into `tools/build-initramfs.py` to install at
    `/bin/test-kabi-mount-ext4` plus pre-create `/mnt/ext4`.
  * Add `make ARCH=arm64 test-kabi-mount-ext4` target.
  * Likely flush a couple more fs-stub / KabiDirectory edges that
    weren't exercised by erofs (ext4's iterate_shared has a
    different on-disk layout — htree directory blocks vs erofs's
    inline dirents).

Phase 13.

## Status

| Phase | Status |
|---|---|
| 8 — inter-module exports | ✅ |
| 9 — load chain | ✅ |
| 10 — ext4 init_module = 0 | ✅ |
| 11 — block_device synth | ✅ |
| 12 v1 — real submit_bh + disk reads | ✅ |
| 12 v2 — fixture + %pV printk | ✅ |
| 12 v3 — `iget_locked` via `s_op->alloc_inode` | ✅ |
| 12 v4 — post-iget trace | ✅ |
| 12 v5 — narrow post-d_make_root | ✅ |
| 12 v6 — real BH lock semantics | ✅ |
| 12 v7-v8 — diagnostic infrastructure | ✅ |
| **12 v9 — fill_super returns 0** | ✅ |
| 13 — userspace `mount -t ext4` | ⏳ |

## Lessons

The bug was a two-line miss in `fs_synth.rs`.  Finding it required
working breadcrumbs.  Working breadcrumbs required a non-broken
patcher.  And the broken patcher's symptom (a NULL+0x6c4 deref) was
indistinguishable from the symptoms a real loader bug or ext4 layout
sensitivity would have produced — which is why we spent v7+v8 building
better instrumentation tools instead of looking at the patcher itself.

Three meta-takeaways:

  1. **C semantics matter when generating C.**  The dangling
     single-statement-if rule is well known, but it's also exactly
     the kind of thing you don't think about when writing a regex
     patcher.  Always wrap injected statements that need to share
     a conditional.

  2. **`make` dependency chains lie.**  `make build` reports
     "nothing to do" on a clean kernel ELF, even when downstream
     artifacts (initramfs) are stale relative to inputs the
     Makefile doesn't know about.  Fix: explicit dependency from
     `build/testing.arm64.initramfs` to `build/kabi-modules/*.ko`.
     Or: a single command that does both.

  3. **When breadcrumbs don't fire, verify they're in the .ko
     that's actually loaded.**  `cmp` against the running rootfs
     before reaching for hypotheses about reloc handling, PAuth,
     trampolines, etc.  This was a 5-second check that would have
     saved a day.
