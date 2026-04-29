# 296 — Phase 5 v3: erofs's `iterate_shared` returns 0, readdir lists every entry

Phase 5 v3 ships the "iomap-lite" translation layer that lets
erofs's directory iterator work end-to-end on a kABI-mounted
filesystem.  `ls`-equivalent enumeration of `/mnt/erofs` now
returns:

```
.            (synth ino=36, type=Directory)
..           (synth ino=36, type=Directory)
hello.txt    (ino=40, type=Regular, from erofs's iterate_shared)
info.txt     (ino=42, type=Regular, from erofs's iterate_shared)
```

The block: erofs's directory iterator routes through
`inode->i_mapping->a_ops->read_folio` — which in real Linux is
`erofs_read_folio` → iomap → `erofs_map_blocks`, a function
that knows about FLAT_PLAIN / FLAT_INLINE / CHUNK_BASED /
COMPRESSED layouts and translates logical-page-N to a physical
disk offset.  Our kABI synth read_folio just read raw bytes
from the backing file — for our test image's
`EROFS_INODE_FLAT_INLINE` root directory that meant we returned
boot-sector zeros instead of the inline dir entries at byte
0x4a0 of the file.

## The fix

Three pieces, isolated to the kABI shim:

### 1. `kernel/kabi/inode_meta.rs` — KabiInodeMeta side-table

```rust
pub struct KabiInodeMeta {
    pub iloc: u64,           // = nid << 5 (compact inode slot)
    pub i_size: u64,
    pub layout: u8,          // EROFS_INODE_FLAT_PLAIN/INLINE/...
    pub raw_blkaddr: u32,
    pub xattr_isize: u16,
    pub backing_path: String,
}

pub static INODE_META: SpinLock<BTreeMap<usize, KabiInodeMeta>> = ...;

pub fn register_inode_from_nid(
    inode_ptr: usize, nid: u64, backing_path: &str,
) -> Result<KabiInodeMeta, ()>;
```

`register_inode_from_nid` reads the on-disk `erofs_inode_compact`
(32 bytes) at `iloc` from the backing file, parses
`(i_format, i_xattr_icount, i_size, i_u)`, decodes the layout via
`(i_format >> 1) & 0x07`, and inserts the result.  Called from
our `iget5_locked` after the `set` callback.

### 2. `kernel/kabi/fs_stubs.rs` — per-inode address_space + meta registration

Every kABI-allocated inode now gets its OWN 256-byte
`address_space` struct, with `mapping->host = inode`.  The
inode's `i_mapping` points at this per-inode mapping.  When
erofs's `erofs_readdir` sets `buf.mapping = dir->i_mapping`,
each dir's mapping is unique and `mapping->host` lets us
identify the inode.

Plus: `fs_ftype_to_dtype` was returning 0 unconditionally
(stub), causing erofs's filldir to deliver `dt_type=0`
(DT_UNKNOWN) for every entry.  Now it does the FT_* → DT_*
translation.

### 3. `kernel/kabi/filemap.rs::read_cache_folio` — layout-aware

```rust
fn resolve_read_request(mapping, file, index)
    -> (path, physical_offset, inline_shift, inline_size) {
    if let Some(host) = mapping->host {
        if let Some(meta) = INODE_META.lock().get(&host) {
            return translate_offset(meta, index);
        }
    }
    // Fallback: raw read at index*4096 (mount-time superblock path).
}
```

For `EROFS_INODE_FLAT_INLINE`:

```rust
let block_base = (iloc / 4096) * 4096;
let inline_shift = (iloc % 4096) + 32 + xattr_isize;
// Read 4 KiB from file at block_base; copy bytes
// [inline_shift..inline_shift + i_size] to dst[0..i_size];
// zero the tail.
```

For `EROFS_INODE_FLAT_PLAIN`:

```rust
let physical = raw_blkaddr * 4096 + index * 4096;
// Direct read.
```

Other layouts (`COMPRESSED_FULL`, `COMPRESSED_COMPACT`,
`CHUNK_BASED`) defer to Phase 5 v4 / Phase 6.

## Verification

```
kabi: inode_meta[0xffff00007f860810]: nid=36 iloc=0x480 layout=2
      i_size=68 raw_blkaddr=4294967295 xattr_isize=0
kabi: KabiDirectory(erofs).fetch_entries: dispatching iterate_shared
kabi: read_cache_folio: ... INLINE phys=0x0 shift=1184 size=68
kabi: filldir captured: name="hello.txt" ino=40 dt_type=8
kabi: filldir captured: name="info.txt" ino=42 dt_type=8
kabi: KabiDirectory(erofs).fetch_entries: iterate_shared returned 0
kabi: entry[0]: name="." ino=36 type=Directory
kabi: entry[1]: name=".." ino=36 type=Directory
kabi: entry[2]: name="hello.txt" ino=40 type=Regular
kabi: entry[3]: name="info.txt" ino=42 type=Regular
kabi: readdir end at idx=4
```

Note `iterate_shared returned 0` (success, was -EUCLEAN/−117), and
each captured entry has the correct `dt_type` (8 = DT_REG).  The
"shift=1184" log decodes as `0x4a0` — the byte offset of the
first dirent within block 0 of `/lib/test.erofs`.  Erofs's compiled
iterator now sees valid `erofs_dirent` structs at offset 0 of the
folio it requested.

## Why this matters

This isn't just "Phase 5 readdir works."  The translation layer is
the same machinery file reads need.  When Phase 6 wants to read
`hello.txt`, it'll go through the same chain:

  * `KabiFile::read(offset, buf)` → constructs a folio request →
    `read_cache_folio(file_inode->i_mapping, page_index, ...)`.
  * `resolve_read_request` finds the file inode in INODE_META,
    sees `layout=FLAT_PLAIN`, computes physical offset.
  * Reads the right bytes from the backing file.
  * memcpy into the userspace buffer.

So Phase 5 v3 unblocks Phase 6 too — KabiFile read becomes a
straightforward FileLike implementation that reuses
`read_cache_folio` semantics.

## Status

| Item | Status |
|---|---|
| `kabi_mount_filesystem` returns `Ok` | ✅ |
| `KabiFileSystem::root_dir()` | ✅ |
| `KabiDirectory::stat()` | ✅ |
| `KabiDirectory::readdir()` full enumeration | ✅ Phase 5 v3 |
| `KabiDirectory::lookup(name)` | ⏳ v4 — i_op->lookup dispatch |
| `KabiFile::stat()` | ✅ Phase 5 v1 |
| `KabiFile::read()` | ⏳ Phase 6 (unblocked!) |
| Default boot 8/8 LXDE | ✅ |

One commit this session:

  * `ac5d9a4` Phase 5 v3 — `inode_meta.rs` (NEW), per-inode
    address_space, layout-aware `read_cache_folio`,
    `fs_ftype_to_dtype` real impl, `kabi_filldir` `./..` filter.

Total Phase 5 commits: `bea6668` (v1) + `bc5376e` (v2) +
`ac5d9a4` (v3) = three logical iterations, each with explicit
verification. The bring-up loop continues to pay off — every
fix corresponds to a specific Linux struct field, layout flag,
or symbol resolution; the disasm + log instrumentation
pinpoints each within an hour.
