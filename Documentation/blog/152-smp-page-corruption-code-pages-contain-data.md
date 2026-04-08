# Blog 152: SMP Page Corruption — Code Pages Contain Data

**Date:** 2026-04-08

## The Bug

Xorg and dbus-daemon crash intermittently on SMP with SIGSEGV or GPF.
The crashes are 100% reproducible on `-smp 2` over enough runs (~50%
trigger rate) but never occur on `-smp 1`. Both the Xorg GPF at
`ip=0xa102ed976` and the dbus-daemon SIGSEGV at `ip=0xa100e0d20` share
the same root cause: **code pages contain wrong data**.

## Evidence

The dbus-daemon crash provides the clearest evidence. The page fault
handler dumps the instruction bytes at the faulting IP:

```
code at ip=0xa100e0d20: 00 04 00 00 18 00 00 00 ff ff ff ff ff ff ff ff
PAGE FAULT NO VMA: pid=16 addr=0x1420229440 ip=0xa100e0d20
  reason=PageFaultReason(CAUSED_BY_WRITE | CAUSED_BY_USER)
```

The bytes `00 04 00 00 18 00 00 00 ff ff ff ff ff ff ff ff` are clearly
data (two u64 values: `0x0000001800000400` and `0xFFFFFFFFFFFFFFFF`),
not x86 instructions. The CPU interprets `00 04` as `add %al, (%rsp)`
which writes to a computed address (`0x1420229440`) that has no VMA.

These bytes were searched across all shared libraries loaded by
dbus-daemon (libdbus-1, musl, libglib, libgio) — **no match found**.
The data is not from any library's data section. It's from an unknown
source, suggesting the physical page was either:

1. Mapped to the wrong virtual address (page table corruption)
2. Reused from a freed page that still has stale data
3. Overwritten by a concurrent COW or TLB operation

## What's Been Ruled Out

**Disk I/O race:** The virtio-blk driver wraps all operations in a
SpinLock. Two concurrent reads are serialized. The cache is also inside
the lock.

**ext2 read race:** The ext2 `read_block` function uses separate locks
for dirty cache and read cache. The device read is serialized by the
virtio-blk lock.

**Demand paging offset calculation:** The normal-case formula
(`offset_in_file = vma.offset + offset_in_vma`) is correct. The
special case for non-page-aligned VMAs only affects the first page of
a segment.

**Timer/scheduler issues:** All fixed in this session (FMASK, TF
masking, resume_boosted).

## Likely Candidates

### 1. Page allocator double-allocation

If the page allocator returns the same physical page to two different
callers (e.g., one demand fault and one anonymous mmap), both callers
write different data to the same physical memory. The second write
overwrites the first.

### 2. COW copy corruption during fork

When a process forks, shared pages are marked read-only. A write fault
triggers COW: allocate new page, copy old content, map new page. If the
copy source is wrong (stale TLB entry pointing to an already-freed
page), the copy brings garbage data.

### 3. TLB shootdown race

After a page table modification (mprotect, munmap, COW), remote CPUs
must flush their TLB entries. If the flush doesn't complete before the
old physical page is reused, a stale TLB entry on the remote CPU could
read/write the freed page, corrupting whatever was subsequently
allocated there.

### 4. Page cache aliasing

The demand paging page cache (for immutable files like initramfs) maps
`(file_ptr, page_index) → physical_page`. If two different files have
the same pointer value (after one is freed and another allocated at the
same address), the cache would return the wrong page.

## Impact

This bug blocks consistent 4/4 XFCE test results. When the crash
doesn't trigger (~50% of runs), all XFCE components start correctly:
xfce4-session, xfwm4, xfce4-panel, xfconfd.

## Next Steps

1. Add page content verification: after demand-paging a code page,
   compute a checksum. On subsequent faults to the same page (if using
   page cache), verify the checksum matches.
2. Check the page allocator for double-allocation: add a guard pattern
   to freed pages and verify it's intact when re-allocated.
3. Audit the COW path for stale physical page references.
4. Verify TLB shootdown completeness on SMP fork/exec.
