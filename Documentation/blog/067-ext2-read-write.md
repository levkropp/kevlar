# M10 Phase 7: ext2 Read-Write Filesystem

The ext2 driver was read-only. Every write method returned EROFS. Alpine's
`apk` package manager needs to create files, write data, create directories
and symlinks, unlink, rename, and truncate. This was the filesystem blocker
for package management on Kevlar.

## Shared-state architecture fix

The original `Ext2Filesystem` struct held all fields directly. The VFS
`root_dir(&self)` method needs to hand out directory objects that share
mutable state with the filesystem, but it only receives `&self`. The old
code cloned the entire struct into a new `Arc` each time:

```rust
fn root_dir(&self) -> Result<Arc<dyn Directory>> {
    Ok(Arc::new(Ext2Dir {
        fs: Arc::new(Ext2Filesystem {
            device: self.device.clone(),
            superblock: self.superblock.clone(),
            groups: self.groups.clone(),
            // ... every field ...
        }),
        inode: inode,
    }))
}
```

Children didn't share state with each other or the parent. Fatal for
writes: allocating a block in one dir wouldn't be visible to files opened
through another.

The fix splits into `Ext2Filesystem { inner: Arc<Ext2Inner> }`. All
`Ext2Dir`, `Ext2File`, and `Ext2Symlink` instances hold `Arc<Ext2Inner>`
via a cheap clone. Mutable state (group descriptors, free counts) lives in
`Ext2MutableState` behind a `SpinLock`:

```rust
struct Ext2Inner {
    device: Arc<dyn BlockDevice>,
    superblock: Ext2Superblock,
    block_size: usize,
    // ... immutable config ...
    state: SpinLock<Ext2MutableState>,
}

struct Ext2MutableState {
    groups: Vec<Ext2GroupDesc>,
    free_blocks_count: u32,
    free_inodes_count: u32,
}
```

Each file/dir/symlink also wraps its `Ext2Inode` in a `SpinLock` so reads
and writes see consistent state.

## Bitmap allocation

Block and inode allocation scan group descriptor bitmaps for the first free
bit using the same `(!byte).trailing_zeros()` trick from the page allocator:

```rust
fn find_free_bit(bitmap: &[u8], max_bits: usize) -> Option<usize> {
    for (byte_idx, &byte) in bitmap.iter().enumerate() {
        if byte == 0xFF { continue; }
        let bit_in_byte = (!byte).trailing_zeros() as usize;
        let bit = byte_idx * 8 + bit_in_byte;
        if bit < max_bits { return Some(bit); }
    }
    None
}
```

`alloc_block()` iterates groups, reads each bitmap, finds a free bit, sets
it, updates the group descriptor's `free_blocks_count` and the superblock's
global count, then flushes both to disk. Block number =
`group * blocks_per_group + first_data_block + bit_index`.

The lock is dropped during disk I/O (reading/writing the bitmap block) and
re-acquired to update counts. This avoids holding the spinlock across
potentially slow block device operations.

## Block pointer management

New files use ext2-style block pointers (not ext4 extents). This works on
both ext2 and ext4 filesystems since ext4 supports legacy indirect blocks.
Existing extent-based files remain readable; in-place overwrites within
allocated extents work too.

`set_block_ptr()` handles direct blocks (indices 0-11), single indirect
(index 12), and double indirect (index 13). Indirect and double-indirect
blocks are allocated on demand when the file first needs them:

```rust
fn set_block_ptr(&self, inode: &mut Ext2Inode, block_index: usize, block_num: u32) -> Result<()> {
    if block_index < EXT2_NDIR_BLOCKS {
        inode.block[block_index] = block_num;
        return Ok(());
    }

    let index = block_index - EXT2_NDIR_BLOCKS;
    if index < ptrs_per_block {
        if inode.block[EXT2_IND_BLOCK] == 0 {
            let ind = self.alloc_block()? as u32;
            let zero_block = vec![0u8; self.block_size];
            self.write_block(ind as u64, &zero_block)?;
            inode.block[EXT2_IND_BLOCK] = ind;
        }
        // read-modify-write the indirect block ...
    }
    // ... double indirect similarly ...
}
```

## File write

`Ext2File::write()` reads the full user buffer first, then iterates block
by block. For each block in the write range, it resolves the existing block
pointer or allocates a new one. Full blocks are written directly; partial
blocks use read-modify-write. After the loop, `inode.size` is updated if
the file grew.

For extent-based files (ext4), in-place overwrites within existing extents
work. Extending an extent-based file returns ENOSPC — ext4 extent tree
modification is future work.

## Truncate

`Ext2File::truncate()` frees blocks beyond the new size, zeros the partial
tail of the last remaining block, and updates the inode. Block pointers are
cleared as blocks are freed. `i_blocks` (the 512-byte sector count) is
decremented for each freed block.

## Directory mutations

Directory entries use the standard ext2 linked-list format within blocks.
Each entry has `{inode, rec_len, name_len, file_type, name}`. The
`rec_len` field chains entries and absorbs padding.

`add_dir_entry()` walks existing blocks looking for space. When an existing
entry's `rec_len` exceeds its actual size, the entry is shrunk and the new
entry is placed in the freed space. If no block has room, a new block is
allocated and the entry spans it entirely.

`remove_dir_entry()` finds the target by name. If it has a predecessor, the
predecessor's `rec_len` is extended to absorb the removed entry. If it's
the first entry in a block, the inode number is zeroed (marking it as
deleted).

All six directory operations are implemented:

- **create_file** — alloc inode, init as regular file, add dir entry
- **create_dir** — alloc inode, create block with `.`/`..` entries, increment parent links
- **create_symlink** — alloc inode, inline target if <=60 bytes, else allocate data block
- **unlink** — check not dir (EISDIR), remove entry, decrement links, free if zero
- **rmdir** — check empty (ENOTEMPTY), remove entry, free blocks/inode, decrement parent links
- **rename** — same-dir only for MVP (EXDEV for cross-dir), remove old + add new entry
- **link** — add dir entry pointing to existing inode, increment target links

## Group descriptor extension

The read-only driver only parsed `inode_table` from group descriptors. Write
support needs five more fields: `block_bitmap` (offset 0), `inode_bitmap`
(offset 4), `free_blocks_count` (offset 12), `free_inodes_count` (offset
14), `used_dirs_count` (offset 16). All with 64-bit high-word support at
offsets 32/36/40 when `INCOMPAT_64BIT` is set.

`flush_metadata()` writes both the superblock (free counts) and the full
group descriptor table back to disk after every allocation or free. This is
conservative — a write-back cache would batch these — but correct.

## Verification

A 19-test C binary (`testing/test_ext2_rw.c`) exercises every write
operation against a real ext2 image mounted in QEMU:

```
PASS mount_ext2       PASS create_file     PASS write_file
PASS open_for_read    PASS read_file       PASS mkdir
PASS create_in_dir    PASS opendir         PASS readdir_count
PASS symlink          PASS readlink        PASS open_symlink
PASS read_via_symlink PASS unlink          PASS unlinked_gone
PASS truncate         PASS rename          PASS renamed_exists
PASS rmdir
```

An Alpine 3.21 minirootfs ext2 disk image (`make alpine-disk`) with
`apk.static` in the initramfs provides the infrastructure for package
management testing. `apk.static --version` and `--help` work.
`apk.static --root /mnt update` crashes in apk's internal database
parser — the next step is debugging that NULL dereference (ip=0x420000)
which appears to be in apk's tar/blob processing, not a kernel issue.

## Other fixes

- **SIGSEGV diagnostics**: Page fault handler now always logs fault
  address, PID, instruction pointer, and FS base on SIGSEGV — no longer
  hidden behind `debug_assertions`.
- **fstatfs**: Returns correct filesystem type for ext2 paths (was always
  returning tmpfs).
- **statfs ext2**: Updated to report writable (no ST_RDONLY), 4096 block
  size, non-zero free counts.

## Files changed

| File | Change |
|------|--------|
| `services/kevlar_ext2/src/lib.rs` | Full read-write rewrite (938 -> 2094 lines) |
| `services/kevlar_ext2/Cargo.toml` | Add `kevlar_platform` dep for SpinLock |
| `kernel/mm/page_fault.rs` | Always-on SIGSEGV diagnostics |
| `kernel/syscalls/statfs.rs` | Fix fstatfs + ext2 statfs values |
| `testing/Dockerfile` | Alpine ext2 disk image + apk.static + test binary |
| `testing/test_ext2_rw.c` | 19-test ext2 read-write verification suite |
| `testing/test_apk_update.sh` | apk update test script |
| `Makefile` | `alpine-disk`, `run-apk` targets |
