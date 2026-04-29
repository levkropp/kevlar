# 290 — Phase 4 prep: real crc32c + the test fixture's hidden 16 KB blocks

After Phase 3b's `folio_shadow` cleared the inline
`kmap_local_page` blocker, erofs's superblock validation
ran further and immediately surfaced two new things to fix
before Phase 4 proper begins.  Both fell out of disasm-led
investigation of `erofs_read_superblock`'s -EINVAL and
-EBADMSG returns.

## Blocker 1: `blkszbits=14`

`erofs_read_superblock` reads the on-disk byte at offset
`+12` of the superblock (`blkszbits`) and validates:

```asm
4438: cmp w0, #0x3        ; (blkszbits - 9) <= 3 ?
443c: b.hi 46f8           ; else error: "blkszbits %u isn't supported"
4700: mov w21, #-22       ; -EINVAL
```

So valid `blkszbits` is 9..12 (block sizes 512..4096).

Our test image had `blkszbits=14` (16 KB blocks).  Reason:
the fixture was built on macOS with `mkfs.erofs` and no
explicit `-b` flag, so the tool used the host's page size
(16 KB on Apple Silicon).  Linux 7.0's erofs accepts only
512..4096-byte blocks, so the image was unmountable
regardless of how solid the kABI compat layer was.

Fix: regenerate with `mkfs.erofs -d 0 -b 4096`.  And — to
keep this from rotting — generate the fixture inline in
`tools/build-initramfs.py` from the actual test files:

```python
mkfs_erofs = shutil.which("mkfs.erofs")
if mkfs_erofs and not erofs_img_src.exists():
    with tempfile.TemporaryDirectory() as tmp:
        content_dir = Path(tmp) / "erofs-content"
        content_dir.mkdir()
        (content_dir / "hello.txt").write_text(
            "hello from kABI-mounted erofs!\n")
        (content_dir / "info.txt").write_text("kevlar: K33 demo\n")
        subprocess.run([mkfs_erofs, "-d", "0", "-b", "4096",
                        str(erofs_img_src), str(content_dir)],
                       check=True, capture_output=True)
```

The `-b 4096` is now part of the build, not a tribal
detail in someone's terminal history.

## Blocker 2: `crc32c` stubbed to 0

Past the blkszbits check, `erofs_read_superblock`
validates the superblock checksum:

```asm
4600: mov w0, #0xb54a
460c: movk w0, #0x5045, lsl #16   ; w0 = 0x5045_b54a (CRC seed)
4604: sub w2, w2, #0x8            ; len = sb_size - 8
4608: add x1, x21, #0x408         ; data = sb + 8 (skip magic + crc)
4610: bl crc32c
4614: ldr w3, [x19, #4]            ; expected = sb->checksum
4618: cmp w0, w3
461c: b.eq 4454                    ; OK
4630: mov w21, #-74                ; -EBADMSG
```

Our kABI `crc32c` was a stub returning `0`, which never
matches the actual checksum — clean -EBADMSG with
"invalid checksum 0x0, 0x... expected".

Fix: real impl.  Reflected polynomial `0x82F63B78`,
bitwise loop:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn crc32c(crc: u32, data: *const c_void, len: usize) -> u32 {
    if data.is_null() || len == 0 { return crc; }
    let bytes = unsafe { core::slice::from_raw_parts(data as *const u8, len) };
    let mut c = crc;
    for &b in bytes {
        c ^= b as u32;
        for _ in 0..8 {
            let mask = (c & 1).wrapping_neg();
            c = (c >> 1) ^ (0x82F6_3B78 & mask);
        }
    }
    c
}
```

No table, no PMULL hardware acceleration — superblock
validation runs once per mount, not in a hot loop.  When
hot data ends up here we'll pull in the slicing-by-8
table.

## What this enabled

```
kabi: read_cache_folio: fake_page=0xfffffdffc1f36000
      data_va=0xffff00007cd80000 mapping=0x0 index=0
      path=/lib/test.erofs
[no -22 / -74 / fault visible yet; fc_fill_super
 progressed past read_superblock's validation]
```

`erofs_read_superblock` returned 0.  Erofs continued into
the next stage of `fc_fill_super`: blocksize cross-check,
flag bits, then `z_erofs_init_super`, then `erofs_iget`
for the root inode.  Somewhere in there it hit a fault
HVF couldn't classify (`Assertion failed: (isv)`), which
means an exception class arm64's hypervisor doesn't
forward — most likely an alignment or unhandled
synchronous fault on a structure access we haven't
populated.

That's exactly Phase 4: fill in the inode + dentry
struct fields erofs reads at this layer.

## Status

| Blocker | Status |
|---|---|
| Phase 3 v1: read_cache_folio data flow | ✅ |
| Phase 3b: VMEMMAP shadow + TBI1 | ✅ |
| 16 KB-block test image | ✅ (rebuilt with -b 4096) |
| crc32c stub | ✅ (real impl) |
| **Next**: real iget5_locked + d_make_root | ⏳ Phase 4 |

Two committed changes this session:

  * **`Phase 4 prep: real crc32c + auto-generate 4KB-block test fixture`**
    — `kernel/kabi/fs_stubs.rs` (real crc32c),
    `tools/build-initramfs.py` (inline mkfs.erofs invocation
    with `-b 4096`).

The pattern keeps holding: each Phase clears one layer,
the next layer's blocker shows up immediately and
specifically, with disasm pointing at the exact byte
range or function that needs work.  Phase 4 surfaces a
specific function (`erofs_iget`) and a specific
synth (Linux's `struct inode`).  Same iterative
struct-field bring-up that K33 Phase 2c used to find
shrinker_alloc and alloc_workqueue_noprof.
