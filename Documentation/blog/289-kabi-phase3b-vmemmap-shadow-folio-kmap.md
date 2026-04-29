# 289 — Phase 3b: VMEMMAP shadow makes Linux's `kmap_local_page` land

Phase 3 v1 left erofs holding a folio-shaped buffer that it
accepted, dereferenced, and then handed to its inline
`kmap_local_page` arithmetic.  The math produced a VA in
the `0xffbf_802f_xxx` range that we don't map, and the next
read deterministically faulted.

Phase 3b's job: make Linux's compile-time-baked
`kmap_local_page(folio)` land on memory we have.  Two
changes ship — derived constants from disassembling the
exact arithmetic erofs.ko was running, and a tiny
page-table region that satisfies the math.

Net result: `kabi-load-erofs=1 kabi-fill-super=1` now
runs the mount chain through `read_cache_folio`, returns a
folio whose subsequent `kmap_local_page` resolves to a
mapped VA, erofs reads its data buffer cleanly, and the
mount progresses to the next stub layer (the `bdev`
file-open path) before bailing with `-EINVAL`.  Zero
kernel page faults.

## What `kmap_local_page` actually compiles to

Disassembled `erofs_bread+0x138` (offset `0x6c80`) in
`linux-modules-7.0.0-14-generic.deb`'s `erofs.ko`:

```asm
mov   x1, #0x40000000           ; movz lsl #16
movk  x1, #0x200, lsl #32       ; x1 = 0x0200_4000_0000 = -VMEMMAP_START
add   x2, x0, x1                ; x2 = page - VMEMMAP_START
mov   x0, #0xfffffff000000000   ; preshifted PAGE_OFFSET
add   x2, x0, x2, lsr #6        ; x2 = preshift_PAGE_OFFSET + idx
lsl   x2, x2, #12               ; result = PAGE_OFFSET + idx * 4096
```

Decoded constants:

  * `VMEMMAP_START`     = `0xffff_fdff_c000_0000`
  * `PAGE_OFFSET`       = `0xffff_0000_0000_0000`
    **= Kevlar's `KERNEL_BASE_ADDR`**
  * `sizeof(struct page)` = 64

The PAGE_OFFSET match is the lucky break.  Linux on a
48-bit-VA arm64 (the LPA2-fallback path our HVF target
hits) computes runtime PAGE_OFFSET as
`0xffff_0000_0000_0000` — exactly Kevlar's KERNEL_BASE.
That means: if we hand erofs a "fake page" pointer at
`VMEMMAP_START + paddr/64`, Linux's inline math returns
exactly `KERNEL_BASE + paddr` — the Kevlar direct-map VA
where the data already lives.  No data-buffer placement
gymnastics; `kmalloc`-or-`alloc_pages`-returned memory
already sits at the right spot.

## What ships

### Constant table

`kernel/kabi/struct_layouts.rs` gets `LINUX_PAGE_OFFSET`,
`LINUX_VMEMMAP_START`, `LINUX_SIZEOF_STRUCT_PAGE`
documented against the disasm above.

### `kernel/kabi/folio_shadow.rs` (new)

A 4 KiB shadow page mapped at the appropriate VMEMMAP VA
in TTBR1.  One shadow page describes
`4096 / sizeof(struct page) = 64` fake_page entries —
each describing a 4 KiB data page → 256 KiB of paddr
coverage per shadow page.

```rust
pub fn alloc_folio(flags: u64, mapping: *mut c_void, index: u64)
    -> Option<(u64, *mut u8)>
{
    let bump = DATA_BUMP_PADDR.fetch_add(PAGE_SIZE as u64, AcqRel);
    let data_paddr = bump;
    let data_va = PAddr::new(data_paddr as usize)
        .as_vaddr().value() as *mut u8;

    let fake_page_va = VMEMMAP_START + data_paddr / SIZEOF_STRUCT_PAGE;
    unsafe {
        let p = fake_page_va as *mut u8;
        core::ptr::write_volatile(p as *mut u64, flags);
        core::ptr::write_volatile(p.add(16) as *mut *mut c_void, mapping);
        core::ptr::write_volatile(p.add(24) as *mut u64, index);
    }
    Some((fake_page_va, data_va))
}
```

`init()` reserves a contiguous 256 KiB physical
data-buffer pool, computes the corresponding fake_page VA
range (`VMEMMAP_START + data_base/64`), and maps shadow
page(s) at the **computed VA** — not at `VMEMMAP_START`.

The first iteration mapped the shadow at `VMEMMAP_START`
itself.  That was wrong: the data region's paddr is
~2 GiB into RAM, so `paddr/64` lands at ~32 MiB into the
VMEMMAP range, far past one 4 KiB shadow page at offset 0.
First boot deterministically faulted at
`far=0xfffffdffc1f36000` — exactly the fake_page address
for the data region's first paddr, but a VA we hadn't
mapped.  Fix: compute `fake_base = VMEMMAP_START + data_base/64`,
map the shadow page there.

### `kernel/kabi/filemap.rs::read_cache_folio` rewritten

```rust
pub extern "C" fn read_cache_folio(
    mapping: *mut c_void, index: u64,
    _filler: *const c_void, file: *mut c_void,
) -> *mut c_void {
    let path = lookup_synth_file_path_for_mapping(mapping, file)
        .unwrap_or_else(|| String::from("/lib/test.erofs"));

    let (fake_page_va, data_va) = match folio_shadow::alloc_folio(
        folio_shadow::PG_UPTODATE, mapping, index,
    ) {
        Some(t) => t,
        None => return err_ptr_eio(),
    };

    let offset = (index * 4096) as usize;
    if let Err(_) = read_initramfs_at(&path, offset, data_va, 4096) {
        return err_ptr_eio();
    }
    fake_page_va as *mut c_void
}
```

The `kmalloc`-the-folio-buffer-itself approach from v1 is
gone.  Now the folio pointer (`fake_page_va`) is a synth
VA in the shadow region, the data buffer (`data_va`) is
in Kevlar's direct map, and Linux's inline math bridges
them without us touching its arithmetic.

### TBI1 in TCR_EL1

`__tag_set` in Linux 7.0 arm64 places the kasan tag
(`0xff` since the running kernel is built with
`CONFIG_KASAN=n` but `CONFIG_ARM64_MTE=y`) in bits
56-63 of every kmap result.  TCR.TBI1 was `0` in
Kevlar's TCR_EL1, which means kernel VAs with non-zero
top bytes hit translation faults regardless of the lower
bits.

Fix: flip TBI1 to `1`.  TCR_EL1 changes from
`0x000000B5_B5103510` to `0x000000F5_B5103510` (one bit
at position 38).  Kevlar doesn't itself put data in the
top byte of kernel pointers, so this is purely additive.

## Verification

Boot log with `kabi-load-erofs=1 kabi-fill-super=1`:

```
kabi: folio_shadow init: data_paddr=[0x7cdc0000..0x7ce00000)
      fake_page=[0xfffffdffc1f37000..0xfffffdffc1f38000)
      mapped 1 shadow page(s) at VMEMMAP+0x1f37000
      (first_shadow_paddr=0x7e9fc000)
kabi: erofs init_module returned 0
kabi: erofs init_fs_context returned 0 — fc->ops populated
kabi: read_cache_folio: fake_page=0xfffffdffc1f37000
      data_va=0xffff00007cdc0000 mapping=0x0 index=0
      path=/lib/test.erofs
kabi: get_tree_nodev_synth: fill_super returned -22 — bailing
kabi: erofs ops->get_tree returned -22
kabi: erofs mount route returned EINVAL (Phase 3 v1 expected)
```

Compare to Phase 3 v1's terminal state:

```
panic: kernel page fault: pc=0xffff00007c2049d0
                          far=0xffbf_802f_f100_0400
```

Phase 3b: zero faults.  Erofs accepted the fake_page,
performed the kmap math, dereferenced the data buffer,
and progressed past the kmap layer.  The `-EINVAL` is
erofs's `bdev_file_open_by_path` stub returning
`-ENOTBLK` — a separate stub on a separate code path,
unrelated to the page-cache compat that Phase 3b
addresses.

LXDE 8/8 default boot pass.  No regressions.

## Why a tiny page-table change works

The implementation is small because the disasm did the
work upfront.  Once we knew exactly what arithmetic Linux
was running and exactly which constants it wanted, the
code reduced to:

  1. Reserve some paddrs.  256 KiB worth.
  2. Compute where the fake_pages for those paddrs live
     (`VMEMMAP_START + paddr/64`).
  3. Map a shadow page there so writes to fake_page
     headers and reads of fake_page fields don't fault.
  4. Hand erofs `fake_page_va` instead of the data
     buffer.  Linux's math turns it into the data
     buffer's Kevlar VA on its own.

No K35-style page-table aliasing across multi-GiB ranges,
no struct-page reverse-mapping infrastructure.  The
playbook of "pin Linux's exact arithmetic via disasm,
then build the smallest table that satisfies it" keeps
paying out.

## What's left

| Phase | Status |
|---|---|
| 1: sp_el0 task mock | ✅ |
| 2: folio ERR_PTR safety | ✅ |
| 3 v1: read_cache_folio backed by initramfs | ✅ |
| 3b: VMEMMAP shadow + kmap arithmetic | ✅ |
| 4: super_block + inode + dentry struct synth | ⏳ |
| 5: KabiDirectory adapter | ⏳ |
| 6: KabiFile adapter | ⏳ |
| 7: userspace mount(2) integration | ⏳ |

Phase 4 picks up where the EINVAL bails: erofs's bdev
mount path needs `bdev_file_open_by_path` to return a
real-enough `struct file` whose `f_inode->i_bdev` (or
`f_mapping`) erofs can drive.  That's the next stub
surface to fill in — same iterative bring-up approach,
the page-cache layer is now solid underneath.

## Status

| Milestone | Status |
|---|---|
| K30-K33: kABI ext4 control flow | ✅ |
| K34: filp_open synth + get_tree_nodev | ✅ |
| K35: investigation, struct-offset fixes | ✅ |
| Phase 1: sp_el0 task mock | ✅ |
| Phase 2: folio ERR_PTR safety | ✅ |
| Phase 3 v1: read_cache_folio backed by initramfs | ✅ |
| Phase 3b: VMEMMAP shadow + kmap arithmetic | ✅ |
| Phase 4-7: real data flow | ⏳ |

Erofs's compiled `kmap_local_page` now resolves to memory
Kevlar has.  Every kABI shim along the mount chain runs
without faulting; the mount returns errno cleanly when it
hits the next stub.  Phase 4 makes the first stub return
something erofs can dereference.
