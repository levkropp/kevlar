# 298 — Phase 6: `cat /mnt/erofs/hello.txt` returns the right bytes

Phase 6 closes the read path on a kABI-mounted erofs filesystem.
`KabiFile::read(offset, buf, opts)` was the last placeholder in
the chain; today it's a real implementation that translates a
`(file_offset, len)` request into the on-disk bytes of the
backing image.

End-to-end:

```
kabi: stat hello.txt: mode=0o100644 size=31 ino=40
kabi: read hello.txt (31 bytes): "hello from kABI-mounted erofs!\n"
kabi: read hello.txt @6 (16 bytes): "from kABI-mounte"
kabi: read hello.txt @100 = 0 (expect 0)
```

Three reads through `KabiFile`, three correct results.  The
31-byte content matches what `tools/build-initramfs.py` writes
into the test image at build time.

## The design choice: skip erofs's read_iter, translate ourselves

There were two architecturally distinct ways to implement this:

  (A) **Dispatch into erofs's `file->f_op->read_iter`** — synthesise
      `kiocb` + `iov_iter` structs, hand them to
      `erofs_file_read_iter` → `filemap_read` → `a_ops->read_folio`.
      Faithful to the real Linux read path.

  (B) **Translate via `inode_meta` directly, copy to UserBufferMut** —
      use the FLAT_INLINE / FLAT_PLAIN math we already have and
      tested in Phase 5 v3, skip the entire Linux page-cache stack.

Phase 6 picks (B).  Reasons:

  * `inode_meta::translate_offset` already returns
    `(path, physical_offset, inline_shift)` for the only two
    layouts our test image uses.  Identical math powered
    `iterate_shared` in Phase 5 v3 — proven correct.
  * `iov_iter` synthesis is a wide ABI surface (Ubuntu CONFIG-dependent
    field offsets, function-pointer fields for direct/bvec/iovec
    variants); easy to mis-offset.
  * `read_cache_folio` uses `folio_shadow::alloc_folio` — a bump
    allocator with no free path.  File-sized reads would exhaust
    the 256 KiB pool quickly.  Our direct translation reads into
    a 4 KiB stack buffer per chunk, no leak.
  * Approach (A) only really pays off when erofs's translation
    layer does work we can't replicate — compression, chunk-based
    layouts.  Our test image uses neither; defer (A) until needed.

## The read loop

```rust
fn read(&self, offset, buf, _opts) -> VfsResult<usize> {
    let i_size = self.read_inode_field::<i64>(INODE_I_SIZE_OFF) as usize;
    if offset >= i_size { return Ok(0); }
    let want = buf.len().min(i_size - offset);

    let meta = inode_meta::lookup_meta(self.inode).ok_or(EIO)?;
    let mut writer = UserBufWriter::from(buf);
    let mut done = 0;
    let mut kbuf = [0u8; 4096];

    while done < want {
        let file_pos = offset + done;
        let page_idx = (file_pos / 4096) as u64;
        let (path, phys_off, inline_shift) =
            inode_meta::translate_offset(&meta, page_idx).ok_or(EIO)?;

        filemap::read_initramfs_at(&path, phys_off, kbuf.as_mut_ptr(), 4096)?;

        let in_page = file_pos % 4096;
        let src = inline_shift as usize + in_page;
        let chunk = (want - done).min(4096_usize.saturating_sub(src));
        if chunk == 0 { break; }

        writer.write_bytes(&kbuf[src..src + chunk])?;
        done += chunk;
    }
    Ok(done)
}
```

Loop invariants:

  * EOF clamp via `i_size`.  Read past EOF returns 0.
  * Per-page lookup of `(path, physical_offset, inline_shift)` —
    handles both FLAT_INLINE (single page, shift>0) and FLAT_PLAIN
    (multi-page, shift=0).
  * `kbuf` is a stack `[u8; 4096]` — zero allocations, zero leaks.
  * `UserBufWriter::write_bytes` handles both kernel-slice and
    user-VA destinations transparently.

## Why this was a one-shot

Phase 6 worked on the first build + boot run.  No bug-hunting,
no disasm, no instrumentation — the pieces it depends on were
already proven:

  * `inode_meta::translate_offset` — Phase 5 v3, dir-block reads
  * `read_initramfs_at` — Phase 5 v3, called by `read_cache_folio`
  * `UserBufWriter` — used by every `kevlar_tmpfs` read for years
  * `KabiInodeMeta` registration — Phase 5 v3, runs in
    `iget5_locked`

When erofs returned `KabiFile { inode, ... }` from `lookup`,
the inode was already registered in `INODE_META` because
`erofs_iget(sb, hello_nid)` had run our `iget5_locked` shim.
Phase 6 was just plumbing the existing reads into the
`FileLike::read` interface.

## What this completes

| Item | Status |
|---|---|
| `kabi_mount_filesystem` returns `Ok` | ✅ Phase 5 v1 |
| `KabiFileSystem::root_dir()` | ✅ Phase 5 v1 |
| `KabiDirectory::stat() / readdir() / lookup()` | ✅ Phase 5 |
| `KabiFile::stat()` | ✅ Phase 5 v1 |
| `KabiFile::read()` | ✅ **Phase 6** |
| Default boot kernel clean | ✅ |

The kernel-side kABI mount stack is **functionally complete**:
mount → root_dir → readdir → lookup → stat → read all work
end-to-end through erofs.ko's compiled mount machinery + our
side-table for the bytes erofs needs that aren't worth
synthesising as Linux structs.

## What's next

Phase 7: a userspace test program that calls
`mount("/lib/test.erofs", "/mnt/erofs", "erofs", MS_RDONLY, NULL)`,
then walks the directory + reads files via standard libc.  The
syscall side already routes erofs through
`kabi::fs_adapter::kabi_mount_filesystem`; Phase 7 is just
plumbing a C test binary into the initramfs and verifying the
final assertion line:

```
TEST_PASS kabi_mount_erofs
```

The bigger arc — `ext4.ko` mounts, then real Linux drivers,
then GPU stacks — gets unblocked once Phase 7 demonstrates
that "the userspace boundary works."  Phase 6 means the data
path is no longer the question.
