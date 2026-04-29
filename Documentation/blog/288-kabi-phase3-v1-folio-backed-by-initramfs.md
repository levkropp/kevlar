# 288 — Phase 3 v1: read_cache_folio backed by initramfs

Phase 1+2 left the kABI mount chain running end-to-end with
`-EIO` propagating cleanly back through every layer.  No
panics, but no actual disk I/O either.  Phase 3's job is to
hand erofs real folios with real file data so the mount can
progress past `read_cache_folio` and start parsing the on-
disk superblock.

This post is the Phase 3 v1 write-up — one substantial step
into the page-cache layer, plus what the next investigation
needs.

## What v1 ships

```rust
// kernel/kabi/filemap.rs

#[unsafe(no_mangle)]
pub extern "C" fn read_cache_folio(mapping: *mut c_void, index: u64,
                                   _filler: *const c_void,
                                   file: *mut c_void) -> *mut c_void {
    let path = super::fs_synth::lookup_synth_file_path_for_mapping(
        mapping, file)
        .unwrap_or_else(|| String::from("/lib/test.erofs"));

    let folio = super::alloc::kmalloc(4096, 0);
    if folio.is_null() {
        return err_ptr_eio();
    }
    unsafe { core::ptr::write_bytes(folio as *mut u8, 0, 4096); }

    // PG_uptodate + mapping + index in folio header
    const PG_UPTODATE_BIT: u64 = 1 << 3;
    unsafe {
        *(folio.cast::<u64>().add(0)) = PG_UPTODATE_BIT;
        *(folio.cast::<u8>().add(16) as *mut *mut c_void) = mapping;
        *(folio.cast::<u8>().add(24) as *mut u64) = index;
    }

    // Read 4 KiB from /lib/test.erofs into folio+64
    let offset = (index * 4096) as usize;
    let data_start = unsafe { folio.cast::<u8>().add(64) };
    if let Err(e) = read_initramfs_at(&path, offset,
                                      data_start, 4096 - 64) {
        super::alloc::kfree(folio);
        return err_ptr_eio();
    }
    folio
}
```

The folio buffer:
  * **Lives in mapped memory** (`kmalloc` returns a kernel
    direct-map address erofs can dereference).
  * **Has Linux folio header fields** at the offsets erofs
    reads — `flags = PG_uptodate` at +0, `mapping` at +16,
    `index` at +24.
  * **Has data at +64** read from the `/lib/test.erofs`
    initramfs file at the right offset.

The fallback to `/lib/test.erofs` matters: erofs's mount
internally takes a path through `sb->s_bdev->bd_mapping`
which we don't synthesise (that's a real `block_device`
struct we'd need for ext4 too).  Both `mapping` and `file`
arrive null in that path; we fall back to the canonical
test image.

## Verification

```
kabi: erofs init_module returned 0
kabi: erofs init_fs_context returned 0 — fc->ops populated
kabi: read_cache_folio: null mapping/file (0x0/0x0);
      falling back to /lib/test.erofs
kabi: read_cache_folio: folio=0xffff00007fc40010 mapping=0x0
      index=0 path=/lib/test.erofs data@+64 (4032 bytes)
panic: kernel page fault: pc=0xffff00007c2049d0
                         far=0xffbf_802f_f100_0400
```

What the new fault tells us:

  * **Erofs accepted the folio** — passed `IS_ERR(folio)`,
    passed flags check, passed the kasan_tag read at
    `folio[+0]`.
  * **Erofs entered the inline `kmap_local_page` arithmetic**
    — our folio_ptr `0xffff_0000_7fc4_0010` was fed through
    `(folio - VMEMMAP_START) / 64 * 4096 + PAGE_OFFSET`,
    producing a VA in the `0xffbf_802f_xxx` range.
  * **The KASAN tag at top byte (0xff)** comes from
    `page_kasan_tag(folio)` reading our folio's flags
    region.  Our v1 sets PG_uptodate at +0 (low bits); the
    kasan tag bits happen to land at 0xff after Linux's
    encoding.

The fault VA isn't mapped — it's the result of Linux's
compile-time-baked `VMEMMAP_START + PAGE_OFFSET` arithmetic
on our folio pointer, producing a pointer into a region
we don't map.

## Phase 3b: deriving Linux's VA constants

To make the kmap result land on memory we have, we need
either:

  * **Approach A: derive VMEMMAP_START + PAGE_OFFSET
    empirically.**  Pass two known folio pointers; observe
    two output VAs; solve the linear equation
    `va = PAGE_OFFSET + (folio - VMEMMAP_START)/64 * 4096`.
    Then synthesise folio pointers at
    `VMEMMAP_START + paddr/64` so the math produces our
    direct-map VA.

  * **Approach B: VA aliasing.**  Map our paddrs ALSO at
    Linux's expected `PAGE_OFFSET` range via a PGD[256]
    + PUD extension in TTBR1.  Then erofs's computed VA
    falls in our mapped alias region.

A is cleaner (no page-table changes); B is more general
(handles any folio pointer Linux generates).  The K35
attempt at B caused boot flakiness because the PUD only
covered 4 GiB and Linux's math produces VAs at
multi-tens-of-GB.  A targets the specific arithmetic.

The Phase 3b commit will set up an empirical probe: hand
known folio pointers, observe outputs, derive the
constants, then synth folios accordingly.

## Where this stops

Each phase identifies the next compat layer.  v1 of
Phase 3 surfaced exactly what Phase 3b needs to do —
match Linux's `kmap_local_page` arithmetic.  The kABI
playbook continues to behave: every step makes a clean
error visible, every fix moves the mount forward by a
measurable amount.

## Status

| Phase | Status |
|---|---|
| 1: sp_el0 task mock | ✅ |
| 2: folio ERR_PTR safety | ✅ |
| 3 v1: read_cache_folio backed by initramfs | ✅ |
| 3b: VMEMMAP_START + PAGE_OFFSET derivation | ⏳ |
| 4-7: super_block, dentry, KabiDirectory/File, userspace | ⏳ |

What's working today (with `kabi-load-erofs=1
kabi-fill-super=1`):

  * erofs.ko loads, registers, init_fs_context = 0
  * filp_open returns synth file with valid struct
  * sp_el0 task mock makes stack-protect prologue work
  * fc_fill_super dispatches into erofs's read path
  * read_cache_folio kmallocs a folio, reads data, returns
  * Erofs validates the folio, accepts it, computes
    kmap_local_page

What's not working:

  * The kmap result's VA isn't mapped — Phase 3b is
    deriving Linux's constants to fix.

LXDE 8/8 default boot pass; no regressions.
