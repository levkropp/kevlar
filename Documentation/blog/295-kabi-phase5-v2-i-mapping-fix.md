# 295 — Phase 5 v2: `inode->i_mapping` populated, but inline-dir layout still blocks readdir

The Phase 5 v2 plan was: route every kABI-allocated inode's
`i_mapping` to the per-mount synth `address_space` so erofs's
directory iterator picks it up via `dir->i_mapping`.  That fix
landed and the previous "null mapping/file" warning is gone.
But `iterate_shared` still returns `-EUCLEAN` because the deeper
issue is structural: our test image uses **inline-data layout**
for the root directory, and erofs's iterate path needs the
inode's i_mapping to do logical-to-physical offset translation
that our raw-bytes synth doesn't implement.

This blog documents what shipped, why it isn't enough on its
own, and what Phase 5 v3 needs.

## What v2 shipped

`kernel/kabi/fs_stubs.rs::iget5_locked` and `new_inode` now do:

```rust
let mapping = synth_mapping_from_sb(sb);
unsafe {
    *(inode.add(INODE_I_MAPPING_OFF) as *mut *mut c_void) = mapping;
}
```

`synth_mapping_from_sb` walks
`sb->s_fs_info->dif0.file->f_mapping` to find the per-mount
synth `address_space` registered in `SYNTH_FILES`.  Every
kABI-allocated inode now points its `i_mapping` at that.

Verification (boot probe, `kabi-load-erofs=1 kabi-fill-super=1`):

```
before v2:
  kabi: read_cache_folio: null mapping/file (0x0/0x0); falling
        back to /lib/test.erofs
  kabi: read_cache_folio: ... mapping=0x0 index=0 ...

after v2:
  kabi: read_cache_folio: ... mapping=0xffff00007f837210 index=0
        path=/lib/test.erofs
```

Erofs's iterate_shared now hands the right `address_space`
through.  Lookup-by-mapping in `SYNTH_FILES` succeeds, the path
resolves to `/lib/test.erofs`.

## What v2 didn't fix

`iterate_shared` still returns `-117 / -EUCLEAN`.  The reason:
**inline-data directories**.

Our test image's root directory has the
`EROFS_INODE_FLAT_INLINE` data layout (`i_format=0x0004` →
layout=2).  In this layout:

  * The dir's first data block IS the same physical block that
    contains the inode itself.
  * The dir's logical data starts at
    `iloc + sizeof(inode) + xattr_size` — for our root, that's
    physical byte `0x480` (where the dirents begin),
    `0x460` is the inode struct itself.

`fs/erofs/dir.c::erofs_readdir` (line 47) iterates by setting:

```c
buf.mapping = dir->i_mapping;    // now correctly our synth mapping
while (ctx->pos < dir->i_size) {
    erofs_off_t dbstart = ctx->pos - ofs;
    de = erofs_bread(&buf, dbstart, true);
    ...
}
```

For `ctx->pos=0`, `dbstart=0` and erofs expects "block 0 of the
dir" to start with valid `erofs_dirent` structs at byte 0.

In real Linux, the inode's `i_mapping->a_ops->read_folio` for an
inline-layout inode is `erofs_read_folio`, which uses iomap to
translate logical-dir-offset → physical-disk-offset.  For our
inode at `iloc=0x460` with inline data:
  * Logical offset 0 of the dir's data → physical offset 0x480.
  * The page returned for "block 0 of dir" is a buffer whose
    first bytes are the inline dirents (not the boot sector).

Our synth `read_cache_folio` doesn't do this translation.  It
reads raw bytes from `/lib/test.erofs` at `offset = index * 4096`
and hands them back.  For dir block 0, we return bytes 0..4096
of the file (boot sector + sb + inode + dirents + ...), erofs
parses dirents starting at byte 0 (= boot-sector zeros), and
the first `de->nameoff = 0` fails the `nameoff >= sizeof(dirent)`
validation → -EUCLEAN.

## Phase 5 v3 design space

Three viable approaches:

### Option A: Per-inode synth address_space with inline-aware read_folio

Most faithful.  Each kABI-allocated inode gets its own
`address_space`:
  * `a_ops->read_folio` reads the inode's metadata, computes
    the right disk offset for the requested logical page, then
    reads that range from the backing file.
  * For inline data, the first page is `[iloc + isize..iloc + isize + i_size]`
    extracted from the inode's block, padded with zeros.
  * For non-inline (FLAT_PLAIN, CHUNK_BASED, COMPRESSED_*), use
    the inode's `raw_blkaddr` / `chunk_index` to find the disk
    block.

This essentially reimplements `iomap_readpage_iter` +
`erofs_map_blocks` in our shim.  Substantial work but
faithful — calls into erofs's compiled code work as designed.

### Option B: Bypass iterate_shared, parse erofs dir layout in Rust

`KabiDirectory::readdir` could read the inode's metadata
directly via `read_initramfs_at` and parse the erofs dir
format manually.  This sidesteps `iterate_shared`, `i_mapping`,
the iomap layer, etc.

Trade-off: not actually exercising erofs's compiled code
for directory iteration — defeats the
"drop-in Linux replacement" goal partially, though we still
exercise erofs's mount + inode-reading code.

### Option C: Test image without inline data

Use `mkfs.erofs` flags (or a larger directory) to force
non-inline storage.  Then dir block 0 is a dedicated block
containing dirents at byte 0, and our raw-bytes read_folio
serves correctly.

Only kicks the can: real-world erofs images use inline.  But
useful for isolating Phase 5 v3 from the inline question and
shipping `lookup` + `readdir` for non-inline cases first.

## What's next

The natural cadence here is **C → A**: get the simple case
working (non-inline) to validate the Phase 5 plumbing end-to-end,
then extend to inline with Option A (or its smaller cousin: a
shim that handles only inline data via metadata translation,
delegating non-inline reads to the existing path).

Or: skip directly to Phase 6 (KabiFile read) with the existing
mapping, since file data reads via read_folio for non-inline
files would work the same way once we have the translation
logic.

## Status

| Item | Status |
|---|---|
| `inode->i_mapping` populated for kABI inodes | ✅ |
| read_cache_folio gets right mapping in iterate_shared | ✅ |
| `iterate_shared` returns 0 (full readdir) | ⏳ inline-layout block |
| `lookup(name)` | ⏳ blocked on the same translation |
| Default boot 8/8 LXDE | ✅ |

Two commits this session:

  * `bea6668` Phase 5 v1: KabiFileSystem returns Ok, root_dir +
    `.`/`..` work.
  * `bc5376e` Phase 5 v2: i_mapping fix in iget5_locked +
    new_inode.

One blog (this one).

Phase 5 reaches the architectural limit of "single global synth
mapping".  Per-inode address_space synthesis (Option A) is the
right next step but a meaningful chunk of work; Option C
provides a parallel path to validate the rest of the kABI dir
machinery in the meantime.
