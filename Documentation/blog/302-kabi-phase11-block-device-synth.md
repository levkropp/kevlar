# 302 — Phase 11 (ext4 arc): block_device synthesis + ext4_fill_super dispatched

Phase 11 wires the kABI block layer to the existing virtio_blk
driver.  At the end of this commit, ext4.ko's `ext4_fill_super` is
running against a real backing block device — sb fully populated,
bdev valid, sectors readable from `/dev/vda`.  It returns `-ENOMEM`
on its first failed allocation deep inside fill_super; that's Phase
12's input.

End-state log:

```
kabi: ext4 init_module returned 0
kabi: [Phase 11 probe] mount-ext4 via kABI(/dev/vda, MS_RDONLY)
kabi: get_tree_bdev(fc=..., fill_super=...)
kabi: get_tree_bdev: fc->source = "/dev/vda"
kabi: bdev_file_open_by_path("/dev/vda") → file=... bdev=...
kabi: get_tree_bdev: sb=... bdev=... fc_s_fs_info=...
kabi: get_tree_bdev: dispatching fill_super(sb, fc)
kabi: get_tree_bdev: fill_super returned -12
```

## What had to land

Five separate real implementations, each replacing a stub that had
been "just enough to link":

### `bdev_file_open_by_path`

Linux's `bdev_file_open_by_path("/dev/vda", FMODE_READ, holder, hops)`
returns a `struct file *` representing the opened block device.
Our impl:

  * Allocates synth `struct file` (256 B), `struct inode` (1024 B),
    and `struct block_device` (4 KiB) buffers.
  * Sets `file->f_inode = inode` (offset +32, verified Phase 4)
    and `inode->i_sb = host_sb` (Phase 7 fix — avoid NULL deref
    under user process page tables).
  * Sets `inode->i_mode = S_IFBLK | 0o666`, `inode->i_blkbits = 9`
    (512-byte sectors).
  * Registers (file, bdev) in a small side-table so `file_bdev()`
    can recover the bdev pointer later.
  * Returns the synth file pointer.

The inode + bdev are kmalloc'd zero-filled buffers; ext4's
fill_super reads only a handful of fields, none of which we
populate beyond what's listed above.  The non-null pointer + valid
i_sb satisfy ext4's guards.

### `file_bdev`

Look up the side-table entry by file pointer; return the
registered bdev.  ext4 calls this immediately after
`bdev_file_open_by_path` to extract the bdev for `sb->s_bdev`.

### `__bread` / `sb_bread`

This is where the real I/O happens.  `__bread(bdev, blocknr, size)`:

```rust
let device = kevlar_api::driver::block::block_device().unwrap();
let start_sector = blocknr * size / device.sector_size();

let bh = kmalloc(BH_SIZE = 128, GFP_ZERO);
let data = kmalloc(size, GFP_ZERO);

device.read_sectors(start_sector, &mut data[..size])?;

bh[BH_B_STATE_OFF] = BH_UPTODATE;
bh[BH_B_BLOCKNR_OFF] = blocknr;
bh[BH_B_SIZE_OFF] = size;
bh[BH_B_DATA_OFF] = data;
bh[BH_B_BDEV_OFF] = bdev;
return bh;
```

`sb_bread(sb, blocknr)` is a thin wrapper that reads
`sb->s_blocksize` + `sb->s_bdev` and dispatches to `__bread`.

The buffer_head layout (offsets `+0 b_state`, `+24 b_blocknr`,
`+32 b_size`, `+40 b_data`, `+48 b_bdev`) matches Linux 7.0's
`struct buffer_head`.  Filesystems read these offsets directly
without going through any kABI accessor.

### `sb_min_blocksize` + `set_blocksize`

ext4 calls `sb_min_blocksize(sb, 1024)` early in fill_super to set
a "minimum acceptable" block size before reading the real
superblock.  Our impl just sets `sb->s_blocksize = 1024`,
`sb->s_blocksize_bits = 10`, returns 1024.  After ext4 reads the
on-disk superblock and finds the actual block size, it calls
`set_blocksize(bdev, real_size)` and `sb_set_blocksize(sb, real_size)`
to update both.

### `get_tree_bdev`

The kernel-side helper that ext4_get_tree calls (it's literally
just `bl get_tree_bdev`).  Mirrors our existing
`get_tree_nodev_synth` but with bdev wiring:

  1. Read `fc->source` (offset +112, verified) → `"/dev/vda"`.
  2. Call `bdev_file_open_by_path(source, ...)` → synth file +
     bdev pair.
  3. Allocate synth `super_block` (4 KiB).  Pre-populate:
       - `s_blocksize = 1024`, `s_blocksize_bits = 10` (defaults;
         ext4 will reset)
       - `s_maxbytes = i64::MAX`
       - `s_fs_info = fc->s_fs_info` (propagate)
       - `s_bdev = bdev`
  4. SCS-wrap dispatch: `call_with_scs_2(fill_super_ptr, sb, fc)`.
  5. On success, set `fc->root = sb->s_root`, populate
     `LAST_MOUNT_STATE` for `kabi_mount_filesystem`.

Removed the null-returning bulk stub of `get_tree_bdev` from
`ext4_arc_bulk_stubs.rs` and pointed the `ksym!` at the real impl
in `fs_synth.rs`.

## Plus: `kabi_mount_filesystem` actually uses the source arg now

It was hardcoded to `/lib/test.erofs` for erofs's in-kernel probe.
Phase 11 makes it propagate the caller-supplied `source` string,
falling back to the legacy hardcoded path when source is None.
That's how `mount("/dev/vda", ...)` lands in get_tree_bdev with
`fc->source = "/dev/vda"`.

## Why ext4_fill_super still returns -ENOMEM

ext4_fill_super is large — it touches kmem caches, percpu
counters, journal infrastructure, inode caches, dentry caches,
sysfs registration, and the actual superblock / block group
descriptor reads.  Several of those allocators are still in our
auto-generated bulk-stub file returning NULL.  ext4 does
`cbz x0, error` on each result and bails to `-ENOMEM` on the
first failure.

Phase 10 fixed the equivalent failures in `ext4_init_fs`'s
sub-inits (kobject_create_and_add, mempool_create_node_noprof) by
moving them out of the bulk file into real impls.  Phase 12 will
do the same for fill_super's failing allocators — disasm the
function, find the first `cbz x0, error_path`, identify which
stub returned null, replace it.

## Status

| Phase | Status |
|---|---|
| 8 — inter-module exports | ✅ |
| 9 — mbcache + jbd2 + ext4 link | ✅ |
| 10 — ext4 init_module = 0 | ✅ |
| 11 — block_device synth + get_tree_bdev | ✅ |
| 12 — fc_fill_super for ext4 (in-kernel mount) | ⏳ |
| 13 — userspace `mount -t ext4` | ⏳ |

Default boot 8/8 LXDE still clean; Phase 7 erofs test still 8/8 PASS.

## Architectural note

ext4 is now mounting against the SAME virtio_blk device that
`kevlar_ext2` (Kevlar's native ext2/3/4 driver) uses.  Once Phase
12 + 13 land, we'll have two mount paths to the same on-disk
filesystem: the native Rust one and the kABI-wrapped Linux ext4.ko
one.  That's the proof point — "drop-in Linux replacement" really
does mean we can swap a Linux .ko in for our native code at the
filesystem boundary.
