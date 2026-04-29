# 285 — kABI K34: synthesised structs, struct walks, and the VA-layout wall

K33 ended with the kABI mount control flow running cleanly:
erofs.ko's `init_module → init_fs_context → ops->get_tree`
chain dispatched into our stubs and returned errno
gracefully back through.  K34 was budgeted as a 7-day sprint
to bridge from "control flow works" to "ls /mnt/erofs shows
hello.txt + info.txt": synthesise the Linux structs that
erofs's mount machinery dereferences, drive `fc_fill_super`,
walk the resulting dentry tree.

Two days in, the work hits a fundamental obstacle that
isn't 7 days of struct synthesis — it's a **VA-layout
mismatch** between Kevlar and Linux's compiled erofs.ko.
This post writes up what we found, what we shipped along
the way, and where K35+ picks up.

## Day 1: filp_open returns a real struct file

K33 left `filp_open` returning `ERR_PTR(-ENOENT)`.  Day 1
synthesises a real `struct file` with the fields erofs
actually reads:

```rust
let file_buf    = kmalloc(FILE_SIZE, 0);   // 256 bytes
let inode_buf   = kmalloc(INODE_SIZE, 0);  // 1024 bytes
let mapping_buf = kmalloc(AS_SIZE, 0);     // 256 bytes

// struct file fields
*(file_buf + FILE_F_MAPPING_OFF /* 16 */) = mapping_buf;
*(file_buf + FILE_F_INODE_OFF   /* 32 */) = inode_buf;

// struct inode fields
*(inode_buf + INODE_I_MODE_OFF /* 0 */) = S_IFREG | 0o644;
*(inode_buf + INODE_I_SIZE_OFF /* 80 */) = file_size;

// struct address_space fields
*(mapping_buf + AS_HOST_OFF  /* 0   */) = inode_buf;
*(mapping_buf + AS_A_OPS_OFF /* 104 */) = kabi_aops_addr;
```

Plus a side-table mapping `file *` → backing path so
`read_folio` can later look up the initramfs file.

Two layout subtleties came up:

  1.  `AS_A_OPS_OFF` started at 96 and erofs read null —
      address_space's `invalidate_lock` is a `struct
      rw_semaphore` which is 40 bytes (not 32) when
      `CONFIG_RWSEM_SPIN_ON_OWNER=y` (which Ubuntu's stock
      arm64 config has), so `a_ops` lands at 104 not 96.
      Fixed by walking the header layout against the
      matching kernel config.

  2.  The aops table itself was first declared as
      `pub static [usize; N]` and erofs read 0 for
      `read_folio`.  Turned out the static was being
      const-evaluated to different addresses at different
      call sites — `&raw const KABI_AOPS` from
      `init_synth()` and from `filp_open_synth()` produced
      pointers that differed by 0x200 bytes.  Bizarre Rust
      compiler behavior; switched to a heap-allocated
      table whose pointer lives in a `static AtomicUsize`.

Output:

```
kabi: filp_open_synth: file=0xffff00007fc36e10 inode=...
      mapping=... size=16384 for /lib/test.erofs
kabi: filp_open_synth verify: f_mapping=... f_inode=...
      i_mode=0o100644 a_ops=... read_folio=0xffff00004020168c
kabi: erofs ops->get_tree returned -22
```

The shift from -15 (-ENOTBLK from get_tree_bdev_flags) to
-22 (-EINVAL from get_tree_nodev) confirms erofs passed
the `S_ISREG && a_ops->read_folio` check and dispatched
into get_tree_nodev — the next thing to implement.

## Day 2: get_tree_nodev_synth + the wall

`get_tree_nodev` is supposed to allocate an anonymous
`super_block`, call `fill_super(sb, fc)`, and set
`fc->root` from the result.  Implement it:

```rust
let sb = kmalloc(SB_SIZE, 0);  // 4 KiB
write_bytes(sb, 0, SB_SIZE);   // zero-fill
*(sb + SB_S_BLOCKSIZE_OFF) = 4096;
*(sb + SB_S_BLOCKSIZE_BITS_OFF) = 12;
*(sb + SB_S_MAXBYTES_OFF) = i64::MAX;

let fill_fn: unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32
    = transmute(fill_super);
let rc = fill_fn(sb, fc);
```

Run it:

```
kabi: get_tree_nodev_synth(fc=0xffff00007fc34010,
                           fill_super=0xffff00007cdc4d10)
kabi: get_tree_nodev_synth: sb=0xffff00007fc3e010, calling fill_super
panicked at platform/arm64/interrupt.rs:136:17:
kernel page fault: pc=0xffff00007cdc49d0
                   far=0xffff_8010_0000_0400
                   esr=0x96000004
```

`pc=0xffff00007cdc49d0` is at .ko offset 0x49d0, which
falls inside `erofs_iget5_set` (88 bytes from offset
0x4988).  The instruction that faulted is doing a load
from `0xffff_8010_0000_0400` — an unmapped address in
*Linux's* kernel direct map, not Kevlar's.

## The VA-layout mismatch

Linux 7.0 arm64 kernels have a configurable
`CONFIG_ARM64_VA_BITS`.  Ubuntu's stock build:

```
CONFIG_ARM64_VA_BITS=52
CONFIG_ARM64_VA_BITS_52=y
CONFIG_PGTABLE_LEVELS=5
```

`PAGE_OFFSET` is computed at compile time:

```c
#define _PAGE_OFFSET(va)   (-(UL(1) << (va)))
#define PAGE_OFFSET        (_PAGE_OFFSET(VA_BITS))
```

For `VA_BITS=52`: `PAGE_OFFSET = -(1ULL << 52) =
0xfff0_0000_0000_0000`.

Kevlar's arm64 kernel base:

```rust
// platform/arm64/mod.rs:296
pub const KERNEL_BASE_ADDR: usize = 0xffff_0000_0000_0000;
```

That's `_PAGE_OFFSET(48)`, the value Linux would use with
`VA_BITS=48`.  **The two direct-map regions don't
overlap.**

When erofs's compiled code does `kmap_local_page(page)`,
it expands to:

```c
// arch/arm64/include/asm/memory.h:407-413
u64 __idx = ((u64)__page - VMEMMAP_START) / sizeof(struct page);
u64 __addr = PAGE_OFFSET + (__idx * PAGE_SIZE);
return (void *)__addr;
```

with `PAGE_OFFSET` and `VMEMMAP_START` baked in as
compile-time immediate values.  The resulting `__addr` is
in `0xfff0_0000_...`-relative space — which doesn't map to
anything in Kevlar's address space.

The FAR `0xffff_8010_0000_0400` in our trap is consistent
with a different VA_BITS interpretation by the running
hardware (HVF on Apple Silicon doesn't necessarily expose
LPA2), but the principle holds: erofs's pointer arithmetic
produces VAs we don't have mapped.

## Why this isn't 1 more day of work

A "fix the next stub" iteration like Phase 2c can't
resolve this.  The bytes of `kmap_local_page` are baked
into erofs.ko's `.text` — we can't intercept them via
kABI.  Three plausible paths forward:

  **A. Realign Kevlar's kernel VA layout to Linux 7.0's
  `PAGE_OFFSET`.**  Move our kernel base from
  `0xffff_0000_0000_0000` to `0xfff0_0000_0000_0000`.
  Touches every kernel-side address calculation, every
  page-table setup, every linker script.  Multi-week
  arm64 mm work.

  **B. Set up alias mappings.**  Keep Kevlar's existing
  layout, but additionally map every page allocated for
  kABI fs use at a Linux-compat VA in `0xfff0_...`.
  Requires extending the arm64 kernel page tables with
  alias entries, plus a hook in `kmalloc`/page-allocator
  for fs-bound allocations.  ~1-2 weeks of careful page
  table work.

  **C. Build a custom erofs.ko with kABI hooks.**  Patch
  erofs's source to call `kabi_kmap_local_page(page)`
  instead of inline-expanding `kmap_local_page`.  Defeats
  the "drop-in Linux replacement" point — we'd no longer
  be loading Ubuntu's stock binary, we'd be loading a
  Kevlar-flavored fork.

K34 proves the kABI playbook for filesystems works at the
control-flow level.  K35+ tackles option B.

## What ships in K34 anyway

Two commits:

  * `7c21091` — Day 1: filp_open returns a synthesised
    struct file.  Erofs's `S_ISREG && a_ops->read_folio`
    checks pass; mount progresses from -ENOTBLK to
    -EINVAL.  New file `kernel/kabi/struct_layouts.rs`
    pins all relevant Linux 7.0 struct field offsets.
    New file `kernel/kabi/fs_synth.rs` allocates
    file + inode + address_space buffers, populates
    fields, side-tables for read_folio lookup.

  * `484aa4e` — Day 2: `get_tree_nodev_synth` allocates
    super_block + dispatches `fill_super`.  Gated behind
    `kabi-fill-super=1` cmdline flag because the call
    crashes on Linux PAGE_OFFSET-relative VAs we don't
    map.

Plus the new `KABI_AOPS_PTR` infrastructure, the kABI
fs side-table for backing-path lookup, and the K34 sprint
plan in `.claude/plans/`.

What's running today (with `kabi-load-erofs=1` cmdline):

```
kabi: loaded /lib/modules/erofs.ko (299737 bytes,
      60 sections, 1045 symbols)
kabi: erofs init_module returned 0
kabi: erofs init_fs_context returned 0
kabi: filp_open_synth("/lib/test.erofs") size=16384
kabi: erofs ops->get_tree dispatch
kabi: get_tree_nodev_synth(fc=..., fill_super=...)
kabi: gated off (need kabi-fill-super=1 + Linux-compat
      VA aliasing).  erofs control flow proven; data
      flow blocked at VA layout.
```

LXDE 8/8 with all this loaded.

## Status

| Milestone | Status |
|---|---|
| K30-K33: kABI ext4 control flow | ✅ |
| K34 Day 1: filp_open synth struct file | ✅ |
| K34 Day 2: get_tree_nodev_synth | ✅ (gated) |
| K34 Day 3-7: actual mount working | ❌ blocked on K35 |
| K35: Linux-compat VA aliasing | ⏳ next |

## Honest accounting

The 7-day sprint plan I wrote at the top of this week
(`.claude/plans/ethereal-nibbling-treehouse.md`) listed
struct synthesis work as Day 1-7.  In reality, the work
to actually mount an erofs image is gated on a deeper
kernel mm task — the VA aliasing — that wasn't visible
from the K33 endpoint.  Days 1-2 surfaced the obstacle
clearly; days 3-7 of "build the dentry walker, the
KabiDirectory, the userspace mount test" are still
useful but only become exercisable once the VA gate is
through.

The path here mirrors what FreeBSD's LinuxKPI took years
to get right for GPU drivers: getting Linux's compiled
binary to run on a kernel whose VA layout doesn't match
its compile-time assumptions takes infrastructure work
that's specific to the host kernel.  K35's scope is
exactly that infrastructure for arm64.

The good news: with K34's commits in, the moment K35
lands the alias mappings, the mount path resumes from
exactly where the gate is — `fill_super` dispatch — and
the rest of the K34 plan (dentry walk, FileLike, userspace
mount test) is just the iterative "stub until init returns
0" work the kABI playbook is good at.
