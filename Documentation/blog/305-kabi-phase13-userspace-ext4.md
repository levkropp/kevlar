# 305 — Phase 13 (ext4 arc): userspace `mount -t ext4` works (7/8) via kABI null-guard

After Phase 12 v9 closed `fill_super` returning 0, Phase 13 took
the kABI ext4 stack from "in-kernel boot probe reads bytes
correctly" to "userspace `mount(2)` + `opendir(2)` + `readdir(2)` +
`open(2)` + `read(2)` all dispatch through the actual syscall
boundary into ext4.ko".  End state:

```
$ make ARCH=arm64 test-kabi-mount-ext4
TEST_START kabi_mount_ext4
PASS mount_ext4
PASS opendir_mnt_ext4
FAIL readdir_hello.txt        ← Phase 13 v3 territory
PASS readdir_info.txt
PASS open_hello.txt
PASS read_hello.txt_size
PASS read_hello.txt_bytes
PASS read_hello.txt_offset_6
RESULTS: 7 passed, 1 failed
```

7/8 from userspace.  The architectural pieces — read_iter dispatch,
real bio infrastructure, mount(2) routing, and a small but
surprisingly load-bearing null-guard page — all line up.

## Phase 13 v1: dispatching `read_iter` end-to-end

The user's call: dispatch into ext4's own `file_operations->read_iter`
rather than rolling a Rust-side ext4 extent decoder.  More
"drop-in" Linux compat: ext4's compiled code handles its own
extents, indirect blocks, and (eventually) compression.  The
cost is synthesising `(struct kiocb, struct iov_iter, struct kvec)`
and implementing enough of the chain underneath to actually do
the read.

The chain that actually had to work:

```
KabiFile::read
  → call_with_scs_2(inode->i_fop->read_iter, &kiocb, &iter)
    → ext4_file_read_iter (in ext4.ko)
      → generic_file_read_iter         (was a NULL-returning bulk stub!)
        → filemap_read                 (was a noop stub!)
          → read_cache_folio           (extended for ext4 dispatch)
            → mapping->a_ops->read_folio
              = ext4_read_folio        (in ext4.ko)
                → ext4_mpage_readpages
                  → bio_alloc_bioset   (was a NULL-returning stub!)
                  → bio_add_folio      (was a NULL-returning stub!)
                  → submit_bio         (was a noop stub!)
                    → block_device.read_sectors
```

Every `(was a stub!)` line was a real bring-up step.

### Generic-file-read-iter and filemap_read

`ext4_file_read_iter` does a few sanity checks and then defers to
`generic_file_read_iter`, which our `kernel/kabi/ext4_arc_bulk_stubs.rs`
had as a null-returning auto-generated stub.  Result: `read_iter`
returned 0 every time, my probe got "0 bytes" instead of the file
contents, and I spent 30 minutes wondering why my carefully-laid
kiocb/iov_iter triple looked correct in a byte dump.

Fix was four lines:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn generic_file_read_iter(
    iocb: *mut c_void, iter: *mut c_void,
) -> isize {
    super::filemap::filemap_read(iocb, iter, 0)
}
```

`filemap_read` itself was also a noop stub.  Wrote a minimal real
impl: loops one folio at a time over the requested range, fetches
each via `read_cache_folio`, copies to the kvec, advances
`kiocb->ki_pos` and the iter state.  KVEC iter only — no IOVEC,
UBUF, or BVEC support.  ~80 LOC.

### read_cache_folio: dispatch into ext4_read_folio

`read_cache_folio`'s erofs-era v1 read raw bytes from the
initramfs file at `index * 4096`.  Ext4's mapping needs
`a_ops->read_folio` (= ext4_read_folio) to handle its on-disk
extent format.  Extended `read_cache_folio` to a three-tier
dispatch:

  1. **`INODE_META`** present → erofs layout decoder (FLAT_PLAIN /
     FLAT_INLINE), unchanged.
  2. **mapping->a_ops->read_folio** present → call it (= ext4
     handles its own extents).
  3. Legacy initramfs raw read fallback for the mount-time
     superblock-read path.

Recursion guard: skip tier 2 if a_ops is the kABI synth aops
(prevents looping back into our own shim).

### Bio infrastructure

`ext4_mpage_readpages` allocates a bio, sets
`bio->bi_iter.bi_sector`, calls `bio_add_folio`, then
`submit_bio`.  All three were stubs.  Wrote a 320-byte bio shim
with inline 8-bvec storage:

```rust
const BIO_TOTAL_SIZE: usize = 320;
const BIO_BVEC_INLINE_OFF: usize = 192;   // bvecs after the bio struct
const BIO_BVEC_COUNT_OFF: usize = 184;    // our scratch vec-count
```

`submit_bio` walks the bvecs, calls
`block_device().read_sectors()` per bvec, marks each folio
`PG_uptodate`, invokes `bi_end_io` if registered.  One alloc per
bio, freed by `bio_put` (or by submit_bio when no end_io).

### The two layout discoveries along the way

**Folio.mapping at +24, not +16.**  Erofs's mount-side code reads
`inode->i_mapping` directly and never touches `folio->mapping`,
so the +16 assumption from K33 worked silently.  ext4_read_folio
reads `folio->mapping->host` as its first action.  Linux 7.0's
struct folio has `flags +0`, then a 16-byte `union { list_head
lru, pgmap }`, then `mapping +24`.  Bumped folio_shadow's
populating offsets and erofs still passed 8/8 — confirms erofs
never relied on the wrong value.

**Ubuntu's struct bio is 8 bytes shorter than mainline.**  Mainline
7.0 has bi_iter at +48 (after `__bi_remaining` + 4 bytes pad +
bi_io_vec at +40); Ubuntu's compiled ext4.ko writes
bi_iter.bi_sector to bio[+40] and bi_end_io to bio[+64] (verified
by disasm of the bio_alloc_bioset call site).  CONFIG difference
shaved off `__bi_remaining + pad`.  Adjusted offsets to match.

## Phase 13 v1 result

End state in the in-kernel boot probe:

```
kabi: KabiFile(ext4).read: dispatching read_iter (want=30 offset=0)
kabi: filemap_read: ki_pos=0 count=30 i_size=30 ...
kabi: read_cache_folio: dispatching a_ops->read_folio
kabi: submit_bio: bio=... start_sector=1104 bvec_count=1
kabi: filemap_read: returning 30 bytes
kabi: ext4 probe: read hello.txt = 30 bytes:
  "hello from kABI-mounted ext4!\n"
EXT4 PROBE: fill_super returned 0 PASS
```

Full chain, real disk read at sector 1104 (= block 138 × 8
sectors/block, the physical block holding hello.txt's content
per debugfs).

Userspace `mount -t ext4` test (commit `8a30f5a`): **2/8 PASS**.
mount + opendir succeed.  readdir crashes the kernel.

## Phase 13 v2: kernel page fault in process VA — and the elegant fix

```
panicked at platform/arm64/interrupt.rs:136:17:
kernel page fault: pc=0xffff00007c085b48, far=0x38, esr=0x96000006
```

`far=0x38` is `NULL+56`.  ESR DFSC=0x06 means translation fault at
level 2 — the L2 page table for VA 0x38 has no valid entry.  PC is
inside ext4.ko, somewhere in `ext4_readdir`.

This was familiar.  Phase 7 had the same shape: ext4 chases
multi-level pointer chains like `(sb[+192])[+56][+64]` everywhere.
The boot probe path runs in **boot context** where TTBR0_EL1
holds the early-init identity map; reads from low VAs return
zero pages of physical RAM silently.  The userspace mount(2) path
runs in **process context** where TTBR0_EL1 is the user's empty
PGD; reads from low VAs translation-fault.

Phase 7 fixed one such site by allocating a fake `host_sb` for
filp_open_synth's chain.  Phase 13 v1 added another for the
mount-time `sb[+192]` chain — got us from 0/8 to 2/8.

Each whack-a-mole patch was load-bearing for one specific code
path.  Every new fs operation surfaced new sites.  The structural
fix that's been sitting in the design space since Phase 7: just
**map a zero page at low VAs in every process's TTBR0**.

### The 80-line patch that did it

`platform/arm64/paging.rs`:

```rust
static KABI_NULL_GUARD_PADDR: AtomicU64 = AtomicU64::new(0);

pub fn init_kabi_null_guard() -> Result<PAddr, PageAllocError> {
    let paddr = alloc_pages(1, AllocPageFlags::KERNEL | DIRTY_OK)?;
    unsafe { paddr.as_mut_ptr::<u8>().write_bytes(0, PAGE_SIZE); }
    KABI_NULL_GUARD_PADDR.store(paddr.value() as u64, Ordering::Release);
    Ok(paddr)
}

fn install_null_guard(pgd: PAddr) -> Result<(), PageAllocError> {
    let guard = kabi_null_guard_paddr().ok_or(...)?;
    let pte = traverse(pgd, UserVAddr::new_unchecked(0), true)?;
    let attrs = DESC_VALID | DESC_PAGE
        | ATTR_IDX_NORMAL | ATTR_SH_ISH | ATTR_AF | ATTR_NG;
    // No ATTR_AP_USER → AP[1]=0 (privileged-only, EL0 no access).
    // No ATTR_AP_RO   → AP[2]=0 (kernel RW).
    *pte = guard.value() as u64 | attrs;
    Ok(())
}
```

`PageTable::new()` calls `install_null_guard` after `allocate_pgd`.
`PageTable::duplicate_from` (the fork path) inherits the mapping
naturally because `duplicate_table` does a deep PGD copy.

The permission encoding is the interesting bit:

  * **AP[1] = 0** (no `ATTR_AP_USER`): EL0 access faults.  Userspace
    `*(int*)NULL = 0` from a C program still segfaults — correct
    Linux semantics preserved.
  * **AP[2] = 0** (no `ATTR_AP_RO`): kernel can both read AND write
    the page.  Reads return zeros (chains succeed); writes
    silently land on the shared page (we accept the
    non-determinism — ext4 might write through a NULL+offset slot,
    and corrupting the shared zero page would be bad if anything
    relied on those bytes, but in practice all such writes are
    "stash a value into a struct field" and nobody reads the
    shared zero page back).

### Result

`make ARCH=arm64 test-kabi-mount-ext4`:

```
PASS mount_ext4
PASS opendir_mnt_ext4
FAIL readdir_hello.txt
PASS readdir_info.txt
PASS open_hello.txt
PASS read_hello.txt_size
PASS read_hello.txt_bytes
PASS read_hello.txt_offset_6
RESULTS: 7 passed, 1 failed
```

7/8 PASS.  Mount, opendir, lookup (open implies it), and read all
work end-to-end through `mount(2)` → kABI → ext4.ko → virtio-blk
disk.

Boot log shows the guard page allocated cleanly:

```
kabi: null guard page allocated at paddr=0x5ee87000
```

No regression: `make ARCH=arm64 test-kabi-mount-erofs` still 8/8
PASS.  Default LXDE boot clean.

## What remains (Phase 13 v3)

`readdir_hello.txt` fails because ext4's `iterate_shared` only
emits ONE dirent — the last one — when called against a 5-entry
non-htree directory:

```
kabi: KabiDirectory(ext4).fetch_entries: dispatching iterate_shared(...)
kabi: filldir captured: name="info.txt" ino=13 dt_type=8
kabi: KabiDirectory(ext4).fetch_entries: iterate_shared returned 0
```

debugfs confirms the on-disk layout has 5 entries: `.`, `..`,
`lost+found`, `hello.txt`, `info.txt` — and dumping the dir block
manually shows them all there with correct rec_lens and names.
Our `kabi_filldir` returns 1 = true = continue (matches Linux 7.0
filldir_t convention); our struct dir_context layout matches
Linux's; our filter only skips `.`/`..`.  Yet ext4 calls our
actor exactly once.

The interesting clue: the entry that DOES come through is the
LAST one on disk.  ext4 might be walking the dir block, skipping
ahead via `ctx->pos` updates we're not making, then emitting only
the entry it lands on.  Phase 13 v3 will narrow it down with
disasm-instrumentation of `ext4_readdir`'s position-tracking
loop.

`lookup("hello.txt")` works fine — the read tests prove it — so
the file is reachable, just not via readdir.

## Lessons

  1. **Linux .ko CONFIG matters for struct layouts.**  Vanilla 7.0's
     struct bio has `__bi_remaining + pad` between the small-int
     header and `bi_io_vec`; Ubuntu's compiled kernel doesn't.
     Verify struct offsets against the actual binary you're going
     to load, not the upstream source.

  2. **Boot-context vs process-context VA is a recurring trap.**
     Anything that "works in the in-kernel probe but crashes from
     userspace" should immediately suggest "low VAs are mapped in
     boot context but not in user processes."  Phase 7's host_sb
     fix and Phase 13 v1's sb[+192] fix were both signposts;
     Phase 13 v2 finally took the structural turn.

  3. **A 4 KiB zero page at user VA 0 is the canonical
     compatibility shim.**  FreeBSD's LinuxKPI does similar.  The
     trick is the permission encoding — kernel-RW + EL0-no-access
     means kABI .ko reads succeed silently while user-side NULL
     deref still gets the segfault it deserves.
