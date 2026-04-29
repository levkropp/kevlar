# 297 — Phase 5 v4: `lookup` works, `stat /mnt/erofs/hello.txt` returns the right inode

Phase 5 v4 ships the last directory-side hookup: dispatching
into erofs's compiled `i_op->lookup` so that
`KabiDirectory::lookup("hello.txt")` returns a real `INode` and
`stat` reads back the true file mode + size from the on-disk
inode.

End state:

```
kabi: lookup ok, is_file=true
kabi: stat hello.txt: mode=0o100644 size=31 ino=40
kabi: lookup("nonexistent"): negative dentry
kabi: lookup(nonexistent) → ENOENT
```

Two bugs surfaced and got fixed; both were ABI-layout mismatches
that the existing fault telemetry exposed cleanly.

## Bug 1 — dentry layout (-ENAMETOOLONG for any name)

First trial run after wiring `call_with_scs_3` + the lookup body:
both `lookup("hello.txt")` and `lookup("nonexistent")` returned
`-36` (`-ENAMETOOLONG`).  The same error for both names ruled
out anything name-content-dependent — the failure had to be in
the early length check.

erofs.ko's `erofs_lookup` prologue at `0x877c`:

```asm
ldr   w1, [x19, #36]      ; w1 = dentry->d_name.len
mov   x0, #0xffffffffffffffdc   ; -36 (-ENAMETOOLONG)
cmp   w1, #0xff           ; if len > 255
b.ls  88e0
                          ; else: return -ENAMETOOLONG
```

Erofs reads `d_name.len` at `[dentry + 36]`.  That means
`d_name` (a `qstr` whose `len` field is at offset +4) is at
`[dentry + 32]` — not `+40` as `struct_layouts.rs` had it.

Our value at `+44` (where we *thought* we were writing `len`)
was 9 — fine.  But erofs read `+36` and got `0xffff0000`
(the upper bytes of the d_parent pointer we wrote at `+32`,
which collided with the real `d_name` slot).  `0xffff0000 > 255`
→ `-ENAMETOOLONG`.

Fixes in `kernel/kabi/struct_layouts.rs`:

| Field | Old offset | New offset | Reason |
|---|---|---|---|
| `DENTRY_D_NAME_OFF` | 40 | **32** | Real Linux 7.0 ABI (verified disasm) |
| `DENTRY_D_PARENT_OFF` | 32 | **64** | Move to private offset (collided with d_name) |
| `DENTRY_D_SB_OFF` | 88 | **72** | Private offset; not read by erofs |
| `DENTRY_D_INODE_OFF` | 56 | 56 | Private; only `d_splice_alias` writes it |

Erofs only reads `d_name` from the dentry — every other field
(`d_parent`, `d_sb`, `d_inode`) is set by us via `d_splice_alias`
or read by us in `KabiFileSystem::root_dir()`.  So as long as
`d_name` matches the real ABI, the rest can live anywhere
consistent.

## Bug 2 — `i_blkbits=0` makes erofs probe 33 blocks past EOF

After Bug 1 fix, lookup dispatched but returned `-117`
(`-EFSCORRUPTED` / `-EUCLEAN`).  Same value for both names →
failure was in the dir-block read, not name comparison.

A side-by-side trace of `read_cache_folio` for readdir vs
lookup:

```
readdir: mapping=0xffff..7f837410 index=0  INLINE shift=1184 size=68  ← OK
lookup:  mapping=0xffff..7f837410 index=33 path=/lib/test.erofs phys=0x21000  ← FAIL
```

Same mapping, but lookup asked for **index 33**.  /lib/test.erofs
is exactly 4 KiB (one block).  Index 33 is 132 KiB past EOF —
our `read_initramfs_at` returned zeros, erofs parsed the zero
bytes as `erofs_dirent`, the `nameoff` validation failed →
`-EFSCORRUPTED`.

Why was erofs asking for index 33?  Disasm of
`erofs_find_target_block.constprop.0` at `0x8164`:

```asm
ldrb  w23, [x1, #134]     ; w23 = dir->i_blkbits
ldr   x24, [x22, #80]     ; x24 = dir->i_size = 68
mov   w21, #0x1
lsl   w0, w21, w23        ; blksz = 1 << blkbits
sub   x24, x24, #0x1
sub   w20, w0, #0x1
orr   x20, x20, x24
add   x20, x20, #0x1      ; round_up(i_size, blksz)
asr   x20, x20, x23       ; iblks = round_up >> blkbits
subs  w20, w20, #0x1      ; back = iblks - 1
```

For `i_size=68`:
  * `blkbits=12` → `blksz=4096`, `iblks=1`, `back=0` (one read at mid=0). ✓
  * `blkbits=0`  → `blksz=1`,    `iblks=68`, `back=67` (binary search starts at mid=33). ✗

Our zeroed inode had `i_blkbits=0` because we never run Linux's
`inode_init_always`, which is what assigns
`inode->i_blkbits = sb->s_blocksize_bits`.  Our `iget5_locked`
just `kmalloc + zero` and sets a few fields by hand.

Fix: set `inode[+134] = 12` in `iget5_locked` and `new_inode`.

Verified by re-running and re-dumping the inode:

```
dir_inode[+128] = 0x000c000000000000   ← byte at +134 = 0x0c (12)
read_cache_folio: mapping=... index=0 path=/lib/test.erofs INLINE shift=1184 size=68
                                ^^^^^^^^^                  ^^^^^^^^^^^^^^^^^^^^^^^^^
                                only one read              correct inline data
d_splice_alias: dentry=... ← inode=0xffff..7f861810
lookup: returned 0x0
lookup("hello.txt"): inode=... i_mode=0o100644
```

## What lookup actually does

`KabiDirectory::lookup(name)` (in `kernel/kabi/kabi_dir.rs`):

  1. `kzalloc` a child dentry (256 B).
  2. `kmalloc` a name buffer + NUL-terminate.
  3. Populate `d_name` qstr at `+32`: `hash=0`, `len`, `name=ptr`.
  4. Set `d_parent` (`+64`) and `d_sb` (`+72`) — our private
     convention; not read by erofs.
  5. Read `parent_inode->i_op` (`+32`), then `i_op->lookup`
     (offset 0 in inode_operations).
  6. SCS-wrap the call:
     `call_with_scs_3(lookup_fn, parent_inode, dentry, 0)`.
     (New helper in `loader.rs` — same `str x18 / mov x18, scs /
     blr / ldr x18` pattern as `call_with_scs_2`, with one more
     register set up.)
  7. Decode the return:
     * `NULL` → caller uses input dentry; `d_splice_alias` set
       its `d_inode`.
     * `ERR_PTR(-N)` → translate to `Errno`.
     * Replacement dentry → use it.
  8. Read `target_dentry->d_inode`.  If null → negative dentry
     → `ENOENT`.
  9. Read `inode->i_mode` (`+0`).  Wrap as
     `INode::Directory(KabiDirectory)` for `S_IFDIR`,
     `INode::FileLike(KabiFile)` for `S_IFREG`.

`d_splice_alias` (in `fs_stubs.rs`) is the minimal real impl:
forward `NULL` and `ERR_PTR`, otherwise set
`dentry->d_inode = inode` and return `NULL`.  No alias-table
walk; we don't have one.

## Why these were quick to find

Both bugs surfaced as a specific errno number (`-36`, `-117`).
The kABI shim converted the negative pointer back to `Errno` for
us, and the disasm of erofs.ko gave the exact instruction +
register that produced it.  An hour each, including the inode
dump pass to confirm `+134=0` then `+134=12`.

The pattern from K33 onward keeps working: every blocker is a
specific Linux struct offset, layout flag, or initialization step
that real Linux does and we don't.  Disasm tells us *what* erofs
expects; the existing fault telemetry tells us *what we have*;
the fix is one or two lines.

## Status

| Item | Status |
|---|---|
| `kabi_mount_filesystem` returns `Ok` | ✅ Phase 5 v1 |
| `KabiFileSystem::root_dir()` | ✅ Phase 5 v1 |
| `KabiDirectory::stat()` | ✅ Phase 5 v1 |
| `KabiDirectory::readdir()` full enumeration | ✅ Phase 5 v3 |
| `KabiDirectory::lookup(name)` | ✅ **Phase 5 v4** |
| `KabiFile::stat()` | ✅ Phase 5 v1 |
| `KabiFile::read()` | ⏳ Phase 6 |
| Default boot kernel clean | ✅ (kABI gated by `kabi-load-erofs=1`) |

Phase 5 is complete.  `mount -t erofs /lib/test.erofs /mnt`
works, `ls /mnt` lists the entries, `stat /mnt/hello.txt`
returns the right `mode` + `size` + `ino`.  Reading the actual
bytes lands in Phase 6.
