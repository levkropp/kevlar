# 292 — Phase 4: erofs.ko's mount completes successfully

This session resolved every blocker between erofs's
`fc_fill_super` magic check and a successful return through
`d_make_root`.  Three bugs in three layers — one in the
allocator, two in struct offsets — and the mount went from
"returns -EINVAL" to "fc->root populated with a real dentry
pointing at a real inode."

The remaining issue is a post-return panic that fires on the
way back up through erofs's get_tree epilogue, after our
get_tree_nodev_synth logs success.

## The wins

### `__GFP_ZERO` was not honored

This was the load-bearing fix.  Erofs's
`erofs_init_fs_context` does:

```c
sbi = kzalloc_obj(*sbi);   // sizeof = 432 bytes
```

`kzalloc_obj` expands to a `__kmalloc_cache_noprof(cache,
GFP_KERNEL_ACCOUNT | __GFP_ZERO, sizeof(*sbi))` call.  Our
shim ignored `gfp` and returned dirty heap memory.

What this meant in practice:

  * Every field in `struct erofs_sb_info` came back
    pre-loaded with whatever pointer-shaped values were in
    the heap from the previous allocations.
  * `sbi->dif0.fsoff` (at sbi+32) held a kernel text VA
    rather than 0.
  * `erofs_init_metabuf` set `metabuf->off = sbi->dif0.fsoff`.
  * `erofs_bread` computed
    `page_index = (offset + metabuf->off) >> 12` ≈ a kernel
    text VA shifted right.
  * `read_cache_folio` got called with a 50 GiB index and
    walked off the end of the data buffer; CRC32C ran past
    the end of RAM at paddr `0x80000000` and triggered a
    synchronous external abort.

The fix: honor `__GFP_ZERO` (bit 8, value `0x100`) in our
`kmalloc`-class shims.

```rust
const __GFP_ZERO: u32 = 0x100;

#[unsafe(no_mangle)]
pub extern "C" fn __kmalloc_cache_noprof(_cache: ..., gfp: u32, size: usize) -> *mut c_void {
    if gfp & __GFP_ZERO != 0 { kzalloc(size, gfp) } else { kmalloc(size, gfp) }
}
```

After the fix, the sbi dump immediately shows the expected
all-zero state with only the expected fields populated:

```
sbi[+0x0..]:  0x0 0x0                       ← dif0.path/fscache: NULL
sbi[+0x10..]: 0xffff00007fc34010 0x0        ← dif0.file: synth file ✓
sbi[+0x20..]: 0x0 0x0                       ← dif0.fsoff = 0 ✓
sbi[+0x40..]: 0x30_0000_0002 0x0            ← opt struct
sbi[+0xb0..]: 0xffff00007fc37690 0x0        ← devs at +176
```

`read_cache_folio` now gets `index=0`.

### `I_NEW` is bit 0 (Linux 7.0)

In Linux 7.0's `include/linux/fs.h`:

```c
__I_NEW = 0U,
I_NEW = (1U << __I_NEW)
```

So `I_NEW = 1`.  Our `iget5_locked` shim was setting
`I_NEW = 1 << 3` (older Linux convention: bit 3).  With
the wrong bit, erofs's `erofs_iget` (at `0x5f30`) saw
`tbnz w2, #0, ...` not taken, never called
`erofs_read_inode` to populate the inode from disk, and
`fc_fill_super`'s `(i_mode & 0xf000) == S_IFDIR` check at
`0x4938` returned `-EINVAL`.

Fixed by changing one constant.

### `sb->s_root` is at `+104`, not `+256`

Disasm of `fc_fill_super` at offset `0x4940`:

```asm
493c: bl     d_make_root
4940: str    x0, [x19, #104]   ; sb->s_root = root_dentry
4944: cbz    x0, 4b28           ; null check
```

Was guessed at 256 in `struct_layouts.rs`.  Real offset
verified via the disasm.  After fixing,
`get_tree_nodev_synth` reads back the dentry erofs wrote
and populates `fc->root` correctly:

```
kabi: get_tree_nodev_synth: fc->root = 0xffff00007fc37410
      — mount succeeded
```

### Defensive: `+reserve-x18` LLVM target feature

Added to `kernel/arch/arm64/arm64.json`'s features.
Linux 7.0 modules built with `CONFIG_SHADOW_CALL_STACK`
treat `x18` as the per-task SCS pointer.  Without
`+reserve-x18`, Rust's codegen could pick `x18` as a
scratch register inside our shim functions, breaking the
.ko's `str x30, [x18], #8` / `ldr x30, [x18, #-8]!`
chain.  We don't observe this happening today, but it
removes a class of latent bugs.

## The progression

```
[boot starts ...]
kabi: erofs init_module returned 0
kabi: dispatching erofs init_fs_context(fc=...)
kabi: erofs init_fs_context returned 0 — fc->ops populated
kabi: dispatching erofs ops->get_tree(fc=...)
kabi: get_tree_bdev_flags (stub) — returning -ENOTBLK
kabi: filp_open_synth("/lib/test.erofs")
kabi: get_tree_nodev_synth(fc=..., fill_super=...)
kabi: super_setup_bdi(sb=...) called
kabi: read_cache_folio: ... index=0 ...                ← sb read
kabi: new_inode: inode=... sb=...                       ← z_erofs managed
kabi: iget5_locked: set_fn returned 0
kabi: iget5_locked: inode=... sb=... data=...           ← root inode
kabi: read_cache_folio: ... index=0 ...                 ← root inode block
kabi: d_make_root: dentry=... inode=...
kabi: fill_super dispatch returned rc=0                 ← SUCCESS
kabi: get_tree_nodev_synth: fill_super returned 0
kabi: get_tree_nodev_synth: fc->root = 0x... — mount succeeded
[panic: synchronous exception in get_tree's tail...]
```

Erofs's mount machinery now runs end-to-end.
`fc_fill_super` populates a valid root dentry, our
adapter wires it into `fc->root`, and the value reaches
the SCS wrapper return.  The post-return panic happens
between `get_tree_nodev_synth` returning 0 and the
SCS wrapper exit log — most likely an autiasp
mismatch in erofs's `fc_get_tree` epilogue or a
NULL-function-pointer call on the unwind path.

## What's next

The post-return panic is the last barrier between "Phase 4
internals work" and "Phase 4 returns Ok cleanly to
`kabi_mount_filesystem`."  Investigation continues:

  * It happens deterministically at the same point in
    every run.
  * Backtrace bottoms out at boot_kernel +
    return-from-mount.
  * Likely candidates: PAC autiasp authentication
    failure, a NULL function pointer reached from
    erofs's get_tree epilogue, or stack/SP corruption
    introduced by our SCS asm.

Once that's resolved, Phase 5 (KabiDirectory) becomes
unblocked: the `fc->root` we now successfully populate
becomes the entry point for `lookup`/`readdir`.

## Status

| Bug / step | Status |
|---|---|
| `__GFP_ZERO` honored | ✅ |
| `I_NEW = 1 << 0` | ✅ |
| `SB_S_ROOT_OFF = 104` | ✅ |
| `+reserve-x18` defensive | ✅ |
| `fc_fill_super` returns 0 | ✅ |
| `fc->root` populated | ✅ |
| Post-return PC=0 panic | ⏳ |
| Phase 5 (KabiDirectory) | ⏳ blocked on above |

Three commits this session:

  * **`Phase 4g: kmalloc shims now honor __GFP_ZERO`**
    — `kernel/kabi/alloc.rs`.  Three entry points,
    one constant, one bit.
  * **`Phase 4h: I_NEW = bit 0 (Linux 7.0) +
    sb->s_root offset (256 → 104)`** —
    `kernel/kabi/fs_stubs.rs` +
    `kernel/kabi/struct_layouts.rs`.  Both verified
    via disasm or Linux header.
  * **`Phase 4: arm64 +reserve-x18`** —
    `kernel/arch/arm64/arm64.json`.

LXDE 8/8 default boot continues to pass.
