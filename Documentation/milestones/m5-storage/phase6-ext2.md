# Phase 6: Read-Only ext2 Filesystem

**Goal:** Implement a read-only ext2 filesystem that mounts from the VirtIO
block device and integrates with the existing VFS. Programs can be loaded
and executed from disk.

## Licensing Note

The ext2 on-disk format is a **data structure specification**, not copyrightable
code. Our implementation is clean-room from the publicly documented format:

- **Primary reference:** FreeBSD `sys/fs/ext2fs/` (BSD-2-Clause)
- **Specification:** "The Second Extended Filesystem" by Dave Poirier
- **Documentation:** OSDev wiki ext2 page, kernel.org ext2 documentation
- **Do NOT reference:** Linux `fs/ext2/` (GPL-2.0-only)

This is the same approach used by FreeBSD, Haiku, FUSE-ext2, and many other
non-GPL implementations.

## ext2 On-Disk Format

### Superblock (offset 1024, size 1024)

```c
struct ext2_super_block {
    uint32_t s_inodes_count;       // total inodes
    uint32_t s_blocks_count;       // total blocks
    uint32_t s_free_blocks_count;
    uint32_t s_free_inodes_count;
    uint32_t s_first_data_block;   // usually 1 for 1K blocks, 0 for 4K
    uint32_t s_log_block_size;     // block_size = 1024 << s_log_block_size
    uint32_t s_blocks_per_group;
    uint32_t s_inodes_per_group;
    uint16_t s_magic;              // 0xEF53
    uint16_t s_state;              // 1=clean, 2=has errors
    uint32_t s_first_ino;          // first non-reserved inode (usually 11)
    uint16_t s_inode_size;         // usually 128 or 256
    // ... more fields
};
```

### Block Group Descriptor (after superblock, one per group)

```c
struct ext2_group_desc {
    uint32_t bg_block_bitmap;      // block bitmap block number
    uint32_t bg_inode_bitmap;      // inode bitmap block number
    uint32_t bg_inode_table;       // first inode table block
    uint16_t bg_free_blocks_count;
    uint16_t bg_free_inodes_count;
    uint16_t bg_used_dirs_count;
    // ... padding
};
```

### Inode (128 or 256 bytes)

```c
struct ext2_inode {
    uint16_t i_mode;         // file type + permissions
    uint16_t i_uid;
    uint32_t i_size;         // file size (low 32 bits)
    uint32_t i_atime, i_ctime, i_mtime, i_dtime;
    uint16_t i_gid;
    uint16_t i_links_count;
    uint32_t i_blocks;       // 512-byte block count
    uint32_t i_block[15];    // block pointers:
    //   [0..11]  direct blocks
    //   [12]     single indirect
    //   [13]     double indirect
    //   [14]     triple indirect
};
```

### Directory Entry

```c
struct ext2_dir_entry_2 {
    uint32_t inode;       // inode number
    uint16_t rec_len;     // entry length (to next entry, with padding)
    uint8_t  name_len;    // actual name length
    uint8_t  file_type;   // EXT2_FT_REG_FILE, EXT2_FT_DIR, etc.
    char     name[];      // filename (NOT NUL-terminated)
};
```

## Design

### Ext2Filesystem

```rust
pub struct Ext2Filesystem {
    device: Arc<dyn BlockDevice>,
    superblock: Ext2Superblock,
    block_size: usize,
    groups: Vec<Ext2GroupDesc>,
    inodes_per_group: u32,
}
```

### Core Operations

1. **Mount:** Read superblock at offset 1024, validate magic (0xEF53), read
   block group descriptors. Store in `Ext2Filesystem`.

2. **Inode lookup:** Given inode number N:
   - Group = (N - 1) / inodes_per_group
   - Index = (N - 1) % inodes_per_group
   - Block = groups[group].bg_inode_table + (index * inode_size) / block_size
   - Offset within block = (index * inode_size) % block_size

3. **Read file data:** Follow block pointers:
   - Blocks 0-11: direct (i_block[0..12])
   - Block 12: single indirect (read block at i_block[12], contains u32 block numbers)
   - Block 13: double indirect (block of blocks of block numbers)
   - Block 14: triple indirect (three levels, unlikely needed for read-only)

4. **Directory lookup:** Read directory inode's data blocks, iterate
   `ext2_dir_entry_2` records, match filename.

5. **Symlink resolution:** If inode `i_size <= 60`, the symlink target is
   stored inline in `i_block[0..14]` (60 bytes). Otherwise, read from data
   blocks.

### VFS Integration

Implement the existing `Directory` and `FileLike` traits:

```rust
struct Ext2Dir {
    fs: Arc<Ext2Filesystem>,
    inode_num: u32,
    inode: Ext2Inode,
}

impl Directory for Ext2Dir {
    fn lookup(&self, name: &str) -> Result<INode> { /* ... */ }
    fn stat(&self) -> Result<Stat> { /* ... */ }
    fn readdir(&self, index: usize) -> Result<Option<DirEntry>> { /* ... */ }
    // create_file, unlink, etc. return EROFS (read-only filesystem)
}

struct Ext2File {
    fs: Arc<Ext2Filesystem>,
    inode_num: u32,
    inode: Ext2Inode,
}

impl FileLike for Ext2File {
    fn read(&self, offset: usize, buf: UserBufferMut, opts: &OpenOptions) -> Result<usize> {
        /* translate offset to block numbers, read via block device */
    }
    fn stat(&self) -> Result<Stat> { /* ... */ }
    fn write(...) -> Result<usize> { Err(EROFS) }
}
```

### Mount Integration

```rust
// In sys_mount:
if fstype == "ext2" {
    let device = find_block_device(source)?;
    let fs = Ext2Filesystem::mount(device)?;
    let root_dir = fs.read_inode(2)?;  // inode 2 is always the root directory
    mount_table.mount(target, root_dir);
}
```

### Ringkernel Placement

The ext2 filesystem is **Services/Ring 2** code:
- Lives in `services/kevlar_ext2/` as a separate crate
- `#![forbid(unsafe_code)]` — all block reads go through the `BlockDevice` trait
- Panic-contained via `catch_unwind` at the VFS boundary
- Only depends on `kevlar_vfs` traits and `BlockDevice` trait

## Scope

**In scope (read-only):**
- Mount ext2 filesystem from VirtIO block device
- Read regular files, directories, symlinks
- Direct, single-indirect, double-indirect block pointers
- File permissions in stat (not enforced)
- Timestamps (atime/ctime/mtime) in stat
- statfs with real block counts from superblock
- Execute programs from ext2 (execve loads from disk)

**Deferred:**
- Write support (block allocation, inode update, directory insertion)
- Journal (ext3/ext4 journaling)
- Extended attributes (xattr)
- Large files (>4 GiB, needs i_size_high from ext2 revision 1)
- Sparse files
- Triple indirect blocks (files >~64 MiB with 4K blocks)

## Reference Sources

- FreeBSD `sys/fs/ext2fs/` (BSD-2-Clause) — full ext2/3/4 implementation
- "The Second Extended Filesystem" by Dave Poirier — format specification
- OSDev wiki: https://wiki.osdev.org/Ext2 — implementation guide
- ext2 documentation in kernel.org docs (format description, not code)

## Testing

Create test disk image in Dockerfile:

```dockerfile
FROM ubuntu:20.04 AS ext2disk
RUN apt-get update && apt-get install -qy e2fsprogs
RUN dd if=/dev/zero of=/disk.img bs=1M count=16
RUN mkfs.ext2 -F /disk.img
# Mount, populate, unmount
RUN mkdir /mnt2 && mount -o loop /disk.img /mnt2 && \
    echo "hello from ext2" > /mnt2/greeting.txt && \
    cp /bin/busybox /mnt2/busybox && \
    umount /mnt2
```

Tests:
- `mount("/dev/vda", "/mnt", "ext2", MS_RDONLY, NULL)` succeeds
- `cat /mnt/greeting.txt` outputs "hello from ext2"
- `ls /mnt/` lists directory contents correctly
- `stat /mnt/greeting.txt` shows correct size, permissions, timestamps
- `statfs /mnt/` returns EXT2_SUPER_MAGIC with correct block counts
- Execute `/mnt/busybox` (if static musl binary) — runs from disk
- Write attempts return EROFS
- Symlinks in ext2 resolve correctly
