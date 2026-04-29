# 294 — Phase 5 v1: kABI mount returns Ok, root_dir + . / .. work

Phase 5's first slice landed.  `kabi_mount_filesystem("erofs", ...)`
now returns `Ok(Arc<KabiFileSystem>)` (was `Err(ENOSYS)` even on
success), and `KabiFileSystem::root_dir()` hands back a real
`Arc<dyn Directory>` whose `stat()` and `readdir(0)`/`readdir(1)`
work end-to-end.

Full `readdir` enumeration of file names is still v2 — erofs's
compiled `iterate_shared` path routes through a different mapping
than the superblock-read path, so our `read_cache_folio` fallback
serves the wrong block and erofs returns -EUCLEAN.  Investigation
continues; the work is bounded.

## What this gets us

The boot-time probe in `kernel/main.rs` exercises the full chain:

```rust
match kabi_mount_filesystem("erofs", None, 0, null) {
    Ok(fs) => {
        let root = fs.root_dir()?;
        for i in 0..16 {
            match root.readdir(i)? { ... }
        }
    }
}
```

Boot log:

```
kabi: erofs mount route Ok — exercising root_dir()
kabi: KabiFileSystem(erofs).root_dir(): dentry=...37410 inode=...60810
kabi: root_dir Ok — listing entries
kabi: entry[0]: name="." ino=36 type=Directory
kabi: entry[1]: name=".." ino=36 type=Directory
kabi: KabiDirectory(erofs).fetch_entries: dispatching
      iterate_shared(file=..., ctx=...)
kabi: read_cache_folio: null mapping/file (...); falling back to
      /lib/test.erofs index=0
kabi: KabiDirectory(erofs).fetch_entries: iterate_shared
      returned -117                                    ← -EUCLEAN
kabi: readdir(2) error: EIO
```

The first three are real: root dentry resolved, root inode stat'd,
`.` and `..` entries synthesised correctly with the real inode
number.

## Architecture

### KabiFileSystem (rewritten)

Three fields: `super_block`, `root_dentry`, `name`, plus an
allocated per-mount `dev_id` for the VFS mount-key table.

```rust
impl FileSystem for KabiFileSystem {
    fn root_dir(&self) -> VfsResult<Arc<dyn Directory>> {
        let inode = unsafe {
            *((self.root_dentry as *const u8)
                .add(DENTRY_D_INODE_OFF) as *const usize)
        };
        Ok(Arc::new(KabiDirectory::new(
            self.root_dentry, inode, self.super_block,
            self.dev_id, self.name.clone(),
        )))
    }
}
```

The `(sb, root_dentry)` pair flows from `get_tree_nodev_synth`
via a `LAST_MOUNT_STATE: SpinLock<Option<(usize, usize)>>`
side-channel — single-mount v1 is fine.

### KabiDirectory (new, `kernel/kabi/kabi_dir.rs`)

Implements `Directory` with:

  * **`stat()`** — direct field reads on the inode struct
    (`i_mode +0`, `i_size +80`, `i_ino +64`).
  * **`readdir(idx)`**:
    - `idx == 0` → synth `.` from `i_ino`.
    - `idx == 1` → synth `..` from `i_ino` (parent not tracked).
    - `idx >= 2` → drive `inode->i_fop->iterate_shared(file, ctx)`
      via SCS-wrapped `call_with_scs_2`, with our
      `kabi_filldir` shim as the `dir_context.actor`.  Captured
      entries cached in a SpinLock.
  * **`lookup(name)`** — Phase 5e placeholder (returns ENOSYS).

### kabi_filldir

Linux's `filldir_t`:

```c
bool (*filldir_t)(struct dir_context *, const char *, int len,
                  loff_t pos, u64 ino, unsigned dt_type);
```

Our shim is `extern "C"`, marshals `(name, len)` into a Rust
`String`, maps `dt_type` (DT_DIR=4 / DT_REG=8 / DT_LNK=10) to
`FileType`, and pushes a `DirEntry` into a global
`FILLDIR_BUFFER` SpinLock.  Returns 1 to keep going.

## What's blocking full readdir

Erofs's iterate_shared path eventually calls
`erofs_read_metabuf(buf, sb, offset, in_metabox)`, which sets
`buf->mapping = sbi->dif0.file->f_mapping`.  In the Phase 5
runs, that path produces `mapping = null` going into
`read_cache_folio`.

Our fallback in `filemap.rs::read_cache_folio` sees null
mapping/file and reads `/lib/test.erofs` at `offset = index * 4096`.
For `index=0`, that's block 0 of the file — which DOES contain
the root inode + inline directory entries.  But erofs's dir
parser then runs validation against the data and gets -EUCLEAN
(structure needs cleaning), suggesting either:

  * The validation expects a specific layout we don't provide,
    or
  * Erofs is computing the dir-block address differently than
    the on-disk layout indicates and expects different data
    at `index=0`.

Phase 5 v2 work: instrument exactly which offset erofs walks
to within the returned data buffer, see which check fails.
Most likely a one-byte struct field that's correct on disk
but read at the wrong offset.

## Phase 5 status

| Step | Status |
|---|---|
| 5a: `kabi_mount_filesystem` returns `Ok(KabiFileSystem)` | ✅ |
| 5b: `root_dir()` returns `KabiDirectory` | ✅ |
| 5c: `KabiDirectory::stat()` | ✅ |
| 5d: `readdir` for `.` and `..` | ✅ |
| 5d: `readdir` for `hello.txt`/`info.txt` | ⏳ -EUCLEAN |
| 5e: `lookup(name)` | ⏳ |
| 5f: `KabiFile` placeholder | ✅ (struct + stat) |
| Default boot 8/8 LXDE | ✅ |

One commit this session: `bea6668` Phase 5a-c + 5d/v1.

## Lessons

The new module pattern works: `kernel/kabi/kabi_dir.rs` clusters
`KabiDirectory` + `KabiFile` + `kabi_filldir` together, with
clear separation from `fs_adapter.rs` (mount routing) and
`fs_synth.rs` (low-level file/inode/sb synthesis).

`LAST_MOUNT_STATE: SpinLock<Option<(usize, usize)>>` as a
side-channel between two phases of the mount machinery is a
working pattern for single-mount v1.  When concurrent mounts
matter, this becomes a per-fs_context map.

The next iteration is the same shape as the bring-up loop that
got us through Phases 3-4: log what erofs is actually reading,
disassemble the validation that returns -EUCLEAN, find the
specific field whose value doesn't match expectations, fix.
