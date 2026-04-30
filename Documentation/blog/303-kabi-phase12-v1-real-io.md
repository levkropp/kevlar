# 303 — Phase 12 v1 (ext4 arc): real block I/O, ext4_fill_super reads /dev/vda

Phase 12 v1 advances ext4.ko's `fill_super` from "returns -ENOMEM
on first allocation" to "reads four blocks from the actual virtio_blk
device, parses the superblock, re-reads at the real block size,
runs validation, and returns -EUCLEAN".  ext4 is now doing real
work against the alpine-lxde disk image.

End-state log:

```
kabi: get_tree_bdev: dispatching fill_super(sb, fc)
kabi: submit_bh READ blocknr=1 size=1024 start_sector=2  → ok
kabi: submit_bh READ blocknr=0 size=4096 start_sector=0  → ok
kabi: submit_bh READ blocknr=1 size=4096 start_sector=8  → ok
kabi: submit_bh READ blocknr=1 size=4096 start_sector=8  → ok
kabi: get_tree_bdev: fill_super returned -117  (-EUCLEAN)
```

The first read at block 1, size 1024 returned bytes
`[00, 00, 01, 00, 00, 00, 04, 00]` — that's `s_inodes_count = 65536,
s_blocks_count_lo = 262144`.  ext4 parsed the superblock, set
`s_blocksize = 4096`, then re-read.  Eventually it bailed with
`-EUCLEAN` (filesystem corrupted, by ext4's lights — likely an
ext2-vs-ext4 feature flag mismatch since the disk is ext2).

## Three substantial fixes

### 1. `submit_bh` — promoted from noop to real I/O entry

ext4's bread chain:

```
bdev_getblk → bh with empty data → ext4_read_bh → __ext4_read_bh →
  → submit_bh(REQ_OP_READ, bh)
```

`submit_bh` is the actual block I/O submission point.  Was a noop;
now reads `bh->b_blocknr`, `bh->b_size`, `bh->b_data`, computes
start_sector via `bh->b_size / sector_size`, calls
`kevlar_api::driver::block::block_device().read_sectors(...)`,
sets `b_state |= BH_Uptodate`, and invokes `bh->b_end_io(bh, 1)`
if registered.

### 2. `bdev_getblk` — populate the bh fields

Symptom that pointed at this: `submit_bh READ blocknr=2129391616
size=1024` — ext4 was passing 2-billion-block requests because our
`bdev_getblk` returned a `struct buffer_head` with uninitialized
fields.  ext4's `__ext4_sb_bread_gfp` does roughly:

```
bh = bdev_getblk(bdev, block, size, gfp);
ext4_read_bh(bh) → submit_bh(0, bh)
```

`submit_bh` then reads `bh->b_blocknr` to know what to read.  Linux
populates `b_blocknr` inside `bdev_getblk`; our stub did not.

Fix: `bdev_getblk` now writes `b_blocknr = block`, `b_size = size`,
`b_bdev = bdev`, and forces `GFP_ZERO` so unset fields (b_state,
b_end_io) are deterministic zeros.

### 3. `flush_work` → noop (Phase 12-specific gotcha)

ext4 calls `flush_work(&sbi->some_work)` during fill_super to
synchronize against the per-fs work struct.  Our `flush_work` did:

```rust
let inner_ptr = (*w).inner;       // ← reads "Linux fields" as our shape
if inner_ptr.is_null() { return 0; }
inner.flush_wq.sleep_signalable_until(...)
```

But ext4.ko initialized the work_struct using Linux's
`INIT_WORK` macro, which writes Linux's `struct work_struct` layout
(data + entry + func — 24 bytes total).  Our `WorkStructShim` has
`inner: *mut WorkInner, func: Option<...>` — completely different
shape.  Our `flush_work` read whatever Linux had at offset 0
(some bookkeeping bits) and dereferenced it as a pointer → fault
at `0xfffffffe00048`.

The right fix isn't to align our shim with Linux's layout (that
would make our K2-K8 modules' work_structs broken).  It's to
**not run flush_work on Linux-shaped structs**.  Since for RO mount
nothing actually queues work, flush is just `return 0`.

A real fix when we add write paths would partition the kABI
work-struct space — Linux-shape structs from .ko code go through
real Linux semantics, our shim structs go through `WorkInner`.

## Plus: bulk alloc-stub conversions

12 functions in `ext4_arc_bulk_stubs.rs` converted from `null_mut()`
to `fake_alloc()` (a 64-byte zero-filled kmalloc):

  pcpu_alloc_noprof, d_alloc, d_instantiate_new, dquot_alloc,
  dquot_alloc_inode, posix_acl_alloc, finish_open, simple_inode_init_ts,
  proc_create_seq_private, proc_create_single_data, bdev_file_open_by_dev,
  iget_locked

Plus `alloc_buffer_head` returns a real 128-byte buffer instead of
NULL.

## What remains for Phase 12 v2

`ext4_fill_super` returns `-EUCLEAN` (-117 = corrupted filesystem
per ext4).  The disk is alpine-lxde.arm64.img, which is **ext2**
(per `file` output: "Linux rev 1.0 ext2 filesystem data (mounted
or unclean)").  ext4.ko's mount path likely fails one of:

  * Feature-flag check (ext4 wants `EXT4_FEATURE_INCOMPAT_FILETYPE`
    or similar that ext2 doesn't set).
  * Clean-shutdown check (the "(mounted or unclean)" might flip a
    bit ext4 reads as "needs journal replay").
  * Inode size check (`s_inode_size = 128` for ext2; ext4 might
    require `≥ 256`).

Phase 12 v2 paths:

  (a) Identify the specific check via disasm-trace + EXT4-fs error
      string.  Fix or skip.
  (b) Build a fresh `mkfs.ext4` fixture (small; analogous to
      test.erofs) attached as `/dev/vdb`.  Skip the ext2-vs-ext4
      mismatch entirely.

Path (b) is cleaner but adds a second virtio-blk drive to QEMU
runs; v1 keeps using the existing image.

## Status

| Phase | Status |
|---|---|
| 8 — inter-module exports | ✅ |
| 9 — load chain | ✅ |
| 10 — ext4 init_module = 0 | ✅ |
| 11 — block_device synth | ✅ |
| 12 v1 — real submit_bh + 4 successful disk reads | ✅ |
| 12 v2 — fill_super returns 0 | ⏳ |
| 13 — userspace `mount -t ext4` | ⏳ |

Default boot 8/8 LXDE clean; Phase 7 erofs test still 8/8 PASS.

The shape of Phase 12 v2 is a known-quantity bring-up loop: pick
a path, fix the next blocker, re-run.  If the iteration count
gets unmanageable, switch to a fresh ext4 fixture and let the
test run against a filesystem ext4.ko has no quibbles with.
