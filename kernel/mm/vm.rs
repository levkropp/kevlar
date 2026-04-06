// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::ctypes::MMapProt;
use crate::fs::inode::FileLike;
use crate::{
    arch::{USER_STACK_TOP, USER_VALLOC_BASE, USER_VALLOC_END},
    result::{Errno, Result},
};
use alloc::sync::Arc;
use alloc::vec::Vec;
use kevlar_platform::{
    address::UserVAddr,
    arch::{PageTable, PAGE_SIZE},
};
use kevlar_utils::alignment::{align_up, is_aligned};

#[derive(Clone)]
pub enum VmAreaType {
    Anonymous,
    File {
        file: Arc<dyn FileLike>,
        offset: usize,
        file_size: usize,
    },
    /// Device memory mapped directly into userspace (e.g., framebuffer BAR).
    /// Pages are identity-mapped from the physical base address.
    DeviceMemory {
        phys_base: usize,
    },
}

impl VmAreaType {
    /// Create an adjusted clone for a sub-range of a VMA.
    /// `shift` is the byte offset from the original VMA start to the new VMA start.
    fn clone_with_shift(&self, shift: usize) -> VmAreaType {
        match self {
            VmAreaType::Anonymous => VmAreaType::Anonymous,
            VmAreaType::File { file, offset, file_size } => {
                VmAreaType::File {
                    file: file.clone(),
                    offset: offset + shift,
                    file_size: file_size.saturating_sub(shift),
                }
            }
            VmAreaType::DeviceMemory { phys_base } => {
                VmAreaType::DeviceMemory {
                    phys_base: phys_base + shift,
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct VmArea {
    start: UserVAddr,
    len: usize,
    area_type: VmAreaType,
    prot: MMapProt,
    is_shared: bool,
}

impl VmArea {
    #[inline(always)]
    pub fn area_type(&self) -> &VmAreaType {
        &self.area_type
    }

    #[inline(always)]
    pub fn prot(&self) -> MMapProt {
        self.prot
    }

    #[inline(always)]
    pub fn is_shared(&self) -> bool {
        self.is_shared
    }

    #[inline(always)]
    pub fn start(&self) -> UserVAddr {
        self.start
    }

    #[inline(always)]
    pub fn end(&self) -> UserVAddr {
        self.start.add(self.len)
    }

    #[inline(always)]
    pub fn offset_in_vma(&self, vaddr: UserVAddr) -> usize {
        debug_assert!(self.contains(vaddr));
        vaddr.value() - self.start.value()
    }

    #[inline(always)]
    pub fn contains(&self, vaddr: UserVAddr) -> bool {
        self.start.value() <= vaddr.value() && vaddr.value() < self.start.value() + self.len
    }

    pub fn overlaps(&self, other: UserVAddr, len: usize) -> bool {
        // Two half-open intervals [a,b) and [c,d) overlap iff a < d && c < b.
        // Using <= on either side would incorrectly mark adjacent ranges as overlapping.
        self.start.value() < other.value() + len && other.value() < self.start.value() + self.len
    }

    /// Extend this VMA by `additional` bytes.
    pub fn extend_by(&mut self, additional: usize) {
        self.len += additional;
    }

    pub fn len(&self) -> usize {
        self.len
    }
}

pub struct Vm {
    page_table: PageTable,
    vm_areas: Vec<VmArea>,
    valloc_next: UserVAddr,
    last_fault_vma_idx: Option<usize>,
    /// Heap tracking: independent of VMA indices (which shift on munmap/mmap).
    heap_bottom: UserVAddr,
    heap_end: UserVAddr,
    /// True if this VM was created by fork (duplicate_table). Vm::Drop
    /// only runs teardown on forked VMs to fix CoW refcount inflation.
    is_forked: bool,
    /// True if this VM was created by ghost_fork (no refcount increments).
    /// Vm::Drop uses a different teardown that doesn't decrement refcounts.
    pub is_ghost_forked: bool,
    /// Virtual addresses of pages made read-only during ghost fork.
    /// Used to restore WRITABLE on the parent's PTEs when the ghost
    /// child exec's/exits. Only populated for ghost-forked VMs.
    pub ghost_cow_addrs: Vec<usize>,
}

impl Vm {
    pub fn new(stack_bottom: UserVAddr, heap_bottom: UserVAddr) -> Result<Vm> {
        debug_assert!(is_aligned(stack_bottom.value(), PAGE_SIZE));
        debug_assert!(is_aligned(heap_bottom.value(), PAGE_SIZE));

        let stack_vma = VmArea {
            start: stack_bottom,
            len: USER_STACK_TOP.value() - stack_bottom.value(),
            area_type: VmAreaType::Anonymous,
            prot: MMapProt::PROT_READ | MMapProt::PROT_WRITE,
            is_shared: false,
        };

        let heap_vma = VmArea {
            start: heap_bottom,
            len: 0,
            area_type: VmAreaType::Anonymous,
            prot: MMapProt::PROT_READ | MMapProt::PROT_WRITE,
            is_shared: false,
        };

        Ok(Vm {
            page_table: PageTable::new()?,
            vm_areas: vec![stack_vma, heap_vma],
            valloc_next: USER_VALLOC_BASE,
            last_fault_vma_idx: None,
            heap_bottom: heap_bottom,
            heap_end: heap_bottom,
            is_forked: false,
            is_ghost_forked: false,
            ghost_cow_addrs: Vec::new(),
        })
    }

    #[inline(always)]
    pub fn page_table(&self) -> &PageTable {
        &self.page_table
    }

    #[inline(always)]
    pub fn page_table_mut(&mut self) -> &mut PageTable {
        &mut self.page_table
    }

    pub fn vm_areas(&self) -> &[VmArea] {
        &self.vm_areas
    }

    /// Dump the VMA map to the kernel log for crash diagnosis.
    pub fn dump_vma_map(&self) {
        for vma in &self.vm_areas {
            let end = vma.start().value() + vma.len();
            let prot = vma.prot();
            let r = if prot.contains(MMapProt::PROT_READ) { 'r' } else { '-' };
            let w = if prot.contains(MMapProt::PROT_WRITE) { 'w' } else { '-' };
            let x = if prot.contains(MMapProt::PROT_EXEC) { 'x' } else { '-' };
            let ty = match vma.area_type() {
                VmAreaType::Anonymous => "anon",
                VmAreaType::File { .. } => "file",
                VmAreaType::DeviceMemory { .. } => "dev",
            };
            warn!("  {:012x}-{:012x} {}{}{} {}", vma.start().value(), end, r, w, x, ty);
        }
    }

    #[inline(always)]
    pub fn find_vma_cached(&mut self, vaddr: UserVAddr) -> Option<&VmArea> {
        // Try last successful VMA first (temporal locality optimization)
        if let Some(idx) = self.last_fault_vma_idx {
            if idx < self.vm_areas.len() && self.vm_areas[idx].contains(vaddr) {
                return Some(&self.vm_areas[idx]);
            }
        }

        // Cache miss - do full linear search
        for (i, vma) in self.vm_areas.iter().enumerate() {
            if vma.contains(vaddr) {
                self.last_fault_vma_idx = Some(i);
                return Some(vma);
            }
        }

        self.last_fault_vma_idx = None;
        None
    }

    /// Returns the length of the VMA starting at the given address, if any.
    pub fn vma_len_at(&self, vaddr: UserVAddr) -> Option<usize> {
        self.vm_areas.iter()
            .find(|vma| vma.start() == vaddr)
            .map(|vma| vma.len())
    }

    pub fn last_fault_vma_idx(&self) -> Option<usize> {
        self.last_fault_vma_idx
    }

    fn stack_vma(&self) -> &VmArea {
        &self.vm_areas[0]
    }

    /// Try to grow the stack VMA downward to cover `fault_addr`.
    /// Returns true if the stack was grown, false otherwise.
    /// Linux auto-grows the stack when a fault occurs within a page below
    /// the current stack VMA, up to the stack size rlimit (default 8MB).
    pub fn try_grow_stack(&mut self, fault_addr: UserVAddr) -> bool {
        let stack = &self.vm_areas[0];
        // Only grow if the fault is just below the stack VMA.
        let stack_start = stack.start().value();
        let fault_val = fault_addr.value();
        // Must be below the stack start (stack grows downward).
        if fault_val >= stack_start {
            return false;
        }
        // Must be anonymous RW (a real stack VMA).
        if !matches!(stack.area_type(), VmAreaType::Anonymous) {
            return false;
        }
        // Limit: don't grow more than 8MB below the stack top.
        const MAX_STACK_SIZE: usize = 8 * 1024 * 1024;
        let stack_top = stack.start().value() + stack.len();
        if stack_top - fault_val > MAX_STACK_SIZE {
            return false;
        }
        // Grow the stack VMA down to the fault page (page-aligned).
        let new_start = kevlar_utils::alignment::align_down(fault_val, PAGE_SIZE);
        let growth = stack_start - new_start;
        self.vm_areas[0].start = UserVAddr::new(new_start).unwrap();
        self.vm_areas[0].len += growth;
        true
    }

    /// Update the heap base address (used after loading PIE images).
    pub fn set_heap_bottom(&mut self, new_bottom: UserVAddr) {
        self.heap_bottom = new_bottom;
        self.heap_end = new_bottom;
        // Also update the VMA if it still exists at index 1.
        if self.vm_areas.len() > 1 {
            self.vm_areas[1].start = new_bottom;
            self.vm_areas[1].len = 0;
        }
        // Advance valloc_next past the heap so mmap allocations don't
        // conflict with the heap VMA. Leave a 1-page gap for brk growth.
        let past_heap = new_bottom.add(PAGE_SIZE);
        if past_heap > self.valloc_next {
            self.valloc_next = past_heap;
        }
    }

    pub fn add_vm_area(
        &mut self,
        start: UserVAddr,
        len: usize,
        area_type: VmAreaType,
    ) -> Result<()> {
        self.add_vm_area_with_prot(
            start,
            len,
            area_type,
            MMapProt::PROT_READ | MMapProt::PROT_WRITE | MMapProt::PROT_EXEC,
            false,
        )
    }

    pub fn add_vm_area_with_prot(
        &mut self,
        start: UserVAddr,
        len: usize,
        area_type: VmAreaType,
        prot: MMapProt,
        shared: bool,
    ) -> Result<()> {
        start.access_ok(len)?;

        if !self.is_free_vaddr_range(start, len) {
            warn!("add_vm_area: OVERLAP detected! {:#x}-{:#x} prot={:#x} conflicts with:",
                  start.value(), start.value() + len, prot.bits());
            for vma in &self.vm_areas {
                if vma.overlaps(start, len) {
                    warn!("  existing: {:#x}-{:#x} prot={:#x}",
                          vma.start().value(), vma.start().value() + vma.len(), vma.prot().bits());
                }
            }
            return Err(Errno::EINVAL.into());
        }

        self.vm_areas.push(VmArea {
            start,
            len,
            area_type,
            prot,
            is_shared: shared,
        });

        Ok(())
    }

    pub fn heap_end(&self) -> UserVAddr {
        self.heap_end
    }

    pub fn vm_areas_ref(&self) -> &[VmArea] {
        &self.vm_areas
    }

    pub fn heap_bottom(&self) -> UserVAddr {
        self.heap_bottom
    }

    pub fn valloc_next(&self) -> UserVAddr {
        self.valloc_next
    }

    pub fn set_valloc_next(&mut self, addr: UserVAddr) {
        self.valloc_next = addr;
    }

    pub fn expand_heap_to(&mut self, new_heap_end: UserVAddr) -> Result<()> {
        if new_heap_end < self.heap_bottom {
            return Err(Errno::EINVAL.into());
        }

        if new_heap_end < self.heap_end {
            // Shrink: unmap pages in the freed region.
            let free_start = new_heap_end.value();
            let free_end = self.heap_end.value();
            let start_aligned = kevlar_utils::alignment::align_up(free_start, PAGE_SIZE);

            for addr in (start_aligned..free_end).step_by(PAGE_SIZE) {
                if let Ok(uaddr) = UserVAddr::new_nonnull(addr) {
                    if let Some(paddr) = self.page_table.unmap_user_page(uaddr) {
                        self.page_table.flush_tlb_local(uaddr);
                        if kevlar_platform::page_refcount::page_ref_dec(paddr) {
                            kevlar_platform::page_allocator::free_pages(paddr, 1);
                        }
                    }
                }
            }

            self.heap_end = new_heap_end;
            // Shrink or remove the heap VMA covering [new_heap_end, old_end).
            self.remove_vma_range(new_heap_end, free_end - free_start)?;
            return Ok(());
        }

        // Expand: ensure the new region has a VMA for page fault handling.
        let old_end = self.heap_end;
        let aligned_new = UserVAddr::new_nonnull(align_up(new_heap_end.value(), PAGE_SIZE))?;
        let aligned_old = align_up(old_end.value(), PAGE_SIZE);

        if aligned_new.value() > aligned_old {
            let grow = aligned_new.value() - aligned_old;
            // Guard against heap growing into the stack, but only when the heap
            // is below the stack (non-PIE layout). For PIE binaries the heap is
            // in the valloc region above the stack, so use valloc end instead.
            let stack_bottom = self.stack_vma().start();
            let limit = if self.heap_bottom >= stack_bottom {
                USER_VALLOC_END
            } else {
                stack_bottom
            };
            if aligned_new >= limit {
                return Err(Errno::ENOMEM.into());
            }
            // Add an anonymous VMA for the new heap pages. If the range overlaps
            // an existing VMA (e.g. from a previous brk expansion), try to extend
            // the existing VMA instead. Silently succeed if already covered —
            // brk must never fail once the address is within limits.
            let start = UserVAddr::new_nonnull(aligned_old)?;
            if self.is_free_vaddr_range(start, grow) {
                self.add_vm_area(start, grow, VmAreaType::Anonymous)?;
            } else {
                // Try to extend the existing heap VMA that ends at aligned_old,
                // but ONLY if the extension range doesn't overlap other VMAs.
                let extend_start = UserVAddr::new_nonnull(aligned_old).ok();
                let can_extend = extend_start.map_or(false, |es| {
                    // Check that [aligned_old, aligned_old+grow) is free,
                    // ignoring the VMA we're about to extend.
                    self.vm_areas.iter().all(|area| {
                        let area_end = area.start().value() + area.len();
                        // Skip the VMA we'd extend (it ends at aligned_old)
                        if area_end == aligned_old && matches!(area.area_type(), VmAreaType::Anonymous) {
                            return true; // don't count this one as conflicting
                        }
                        !area.overlaps(es, grow)
                    })
                });
                if can_extend {
                    let _extended = self.vm_areas.iter_mut().any(|area| {
                        let area_end = area.start().value() + area.len();
                        area_end == aligned_old && matches!(area.area_type(), VmAreaType::Anonymous) && {
                            area.extend_by(grow);
                            true
                        }
                    });
                }
                // If we can't extend (would overlap), skip — the heap_end
                // is still advanced but the conflicting VMA keeps its mapping.
                // This matches Linux's behavior where brk into mmap'd regions
                // is silently accepted.
            }
        }
        self.heap_end = new_heap_end;
        Ok(())
    }

    pub fn expand_heap_by(&mut self, increment: usize) -> Result<()> {
        let increment = align_up(increment, PAGE_SIZE);
        let new_end = self.heap_end.add(increment);
        self.expand_heap_to(new_end)
    }

    pub fn fork(&mut self) -> Result<Vm> {
        Ok(Vm {
            page_table: PageTable::duplicate_from(&mut self.page_table)?,
            vm_areas: self.vm_areas.clone(),
            valloc_next: self.valloc_next,
            last_fault_vma_idx: self.last_fault_vma_idx,
            heap_bottom: self.heap_bottom,
            heap_end: self.heap_end,
            is_forked: true,
            is_ghost_forked: false,
            ghost_cow_addrs: Vec::new(),
        })
    }

    /// Ghost-fork: duplicate page table structure but skip refcount
    /// operations. The parent must be blocked until the child exec's/exits.
    /// Collects addresses of CoW-marked pages for fast targeted restore.
    pub fn ghost_fork(&self) -> Result<Vm> {
        let (page_table, cow_addrs) = PageTable::duplicate_from_ghost(&self.page_table)?;
        Ok(Vm {
            page_table,
            vm_areas: self.vm_areas.clone(),
            valloc_next: self.valloc_next,
            last_fault_vma_idx: self.last_fault_vma_idx,
            heap_bottom: self.heap_bottom,
            heap_end: self.heap_end,
            is_forked: false,
            is_ghost_forked: true,
            ghost_cow_addrs: cow_addrs,
        })
    }

    /// Restore WRITABLE on the PARENT's PTEs using the address list from
    /// the ghost child's Vm. O(writable_pages) instead of O(all_PTEs).
    pub fn restore_writable_with_list(&mut self, addrs: &[usize]) {
        self.page_table.restore_writable_from(addrs);
    }

    /// Remove a VMA region [start, start+len). Splits VMAs at boundaries if needed.
    /// Returns the removed/affected VMAs' prot flags for the region.
    pub fn remove_vma_range(&mut self, start: UserVAddr, len: usize) -> Result<()> {
        let end = start.value() + len;
        let mut new_areas: Vec<VmArea> = Vec::new();
        let mut i = 0;

        while i < self.vm_areas.len() {
            let vma = &self.vm_areas[i];
            let vma_start = vma.start.value();
            let vma_end = vma_start + vma.len;

            if vma_end <= start.value() || vma_start >= end {
                // No overlap — keep as-is.
                i += 1;
                continue;
            }

            // This VMA overlaps with [start, end). Remove it and possibly
            // re-insert trimmed pieces.
            let removed = self.vm_areas.remove(i);

            // Left piece: [vma_start, start) — keeps original offset
            if vma_start < start.value() {
                new_areas.push(VmArea {
                    start: removed.start,
                    len: start.value() - vma_start,
                    area_type: removed.area_type.clone(),
                    prot: removed.prot,
                    is_shared: removed.is_shared,
                });
            }

            // Right piece: [end, vma_end) — offset shifts by (end - vma_start)
            if vma_end > end {
                let shift = end - vma_start;
                new_areas.push(VmArea {
                    start: UserVAddr::new_nonnull(end)?,
                    len: vma_end - end,
                    area_type: removed.area_type.clone_with_shift(shift),
                    prot: removed.prot,
                    is_shared: removed.is_shared,
                });
            }

            // Don't increment i — the next element shifted into position.
        }

        self.vm_areas.extend(new_areas);
        Ok(())
    }

    /// Update protection flags for all VMAs overlapping [start, start+len).
    /// Splits VMAs at boundaries if the overlap is partial.
    pub fn update_prot_range(&mut self, start: UserVAddr, len: usize, new_prot: MMapProt) -> Result<()> {
        let end = start.value() + len;

        // Fast path: if the range exactly covers a single VMA, just update
        // the prot field in-place without splitting or allocating.
        for vma in self.vm_areas.iter_mut() {
            if vma.start.value() == start.value() && vma.len == len {
                vma.prot = new_prot;
                return Ok(());
            }
        }

        // Slow path: range partially overlaps one or more VMAs, need to split.
        let mut new_areas: Vec<VmArea> = Vec::new();
        let mut i = 0;

        while i < self.vm_areas.len() {
            let vma = &self.vm_areas[i];
            let vma_start = vma.start.value();
            let vma_end = vma_start + vma.len;

            if vma_end <= start.value() || vma_start >= end {
                i += 1;
                continue;
            }

            // VMA completely contained in [start, end): update in-place.
            if vma_start >= start.value() && vma_end <= end {
                self.vm_areas[i].prot = new_prot;
                i += 1;
                continue;
            }

            let removed = self.vm_areas.remove(i);

            // Left piece (keeps old prot): [vma_start, start)
            if vma_start < start.value() {
                new_areas.push(VmArea {
                    start: removed.start,
                    len: start.value() - vma_start,
                    area_type: removed.area_type.clone(),
                    prot: removed.prot,
                    is_shared: removed.is_shared,
                });
            }

            // Middle piece (new prot): [max(vma_start, start), min(vma_end, end))
            let mid_start = core::cmp::max(vma_start, start.value());
            let mid_end = core::cmp::min(vma_end, end);
            let mid_shift = mid_start - vma_start;
            new_areas.push(VmArea {
                start: UserVAddr::new_nonnull(mid_start)?,
                len: mid_end - mid_start,
                area_type: removed.area_type.clone_with_shift(mid_shift),
                prot: new_prot,
                is_shared: removed.is_shared,
            });

            // Right piece (keeps old prot): [end, vma_end)
            if vma_end > end {
                let right_shift = end - vma_start;
                new_areas.push(VmArea {
                    start: UserVAddr::new_nonnull(end)?,
                    len: vma_end - end,
                    area_type: removed.area_type.clone_with_shift(right_shift),
                    prot: removed.prot,
                    is_shared: removed.is_shared,
                });
            }

            // Don't increment i — the next element shifted into position.
        }

        self.vm_areas.extend(new_areas);
        Ok(())
    }

    /// Extend a VMA's length. The caller must ensure the extension range is free.
    /// `vma_start` identifies the VMA by its start address.
    pub fn extend_vma(&mut self, vma_start: UserVAddr, additional: usize) -> Result<()> {
        for vma in self.vm_areas.iter_mut() {
            if vma.start == vma_start {
                vma.len += additional;
                return Ok(());
            }
        }
        Err(Errno::ESRCH.into())
    }

    pub fn is_free_vaddr_range(&self, start: UserVAddr, len: usize) -> bool {
        self.vm_areas.iter().all(|area| !area.overlaps(start, len))
    }

    pub fn alloc_vaddr_range(&mut self, len: usize) -> Result<UserVAddr> {
        let aligned_len = align_up(len, PAGE_SIZE);
        // Skip over any existing VMAs that overlap with the candidate range.
        // This handles the case where heap VMA or file-backed VMAs were placed
        // in the valloc region by set_heap_bottom or load_elf_segments.
        loop {
            let next = self.valloc_next;
            let end = next.add(aligned_len);
            if end >= USER_VALLOC_END {
                return Err(Errno::ENOMEM.into());
            }
            if self.is_free_vaddr_range(next, aligned_len) {
                // Clear any stale PTEs in the allocated range.
                let num_pages = aligned_len / PAGE_SIZE;
                let mut cleared = 0usize;
                for i in 0..num_pages {
                    let page_addr = next.add(i * PAGE_SIZE);
                    if self.page_table.is_huge_mapped(page_addr).is_some() {
                        self.page_table.split_huge_page(page_addr);
                    }
                    if let Some(stale) = self.page_table.unmap_user_page(page_addr) {
                        self.page_table.flush_tlb_local(page_addr);
                        if kevlar_platform::page_refcount::page_ref_dec(stale) {
                            kevlar_platform::page_allocator::free_pages(stale, 1);
                        }
                        cleared += 1;
                    }
                }
                if cleared > 0 {
                    log::warn!("alloc_vaddr_range: cleared {} stale PTEs at {:#x}+{:#x}",
                               cleared, next.value(), aligned_len);
                }
                self.valloc_next = end;
                return Ok(next);
            }
            // Advance past the conflicting VMA (page-aligned, since pages at
            // the VMA's page-aligned end may have been prefaulted).
            let mut advanced = false;
            for area in &self.vm_areas {
                if area.overlaps(next, aligned_len) {
                    self.valloc_next = UserVAddr::new(
                        align_up(area.end().value(), PAGE_SIZE)
                    ).unwrap_or(area.end());
                    advanced = true;
                    break;
                }
            }
            if !advanced {
                // Shouldn't happen, but avoid infinite loop.
                self.valloc_next = self.valloc_next.add(PAGE_SIZE);
            }
        }
    }

    /// Allocate a virtual address range with a specific alignment.
    /// Used for large anonymous mappings to enable 2MB huge pages.
    pub fn alloc_vaddr_range_aligned(&mut self, len: usize, align: usize) -> Result<UserVAddr> {
        let aligned_next = UserVAddr::new(align_up(self.valloc_next.value(), align))
            .ok_or(Errno::ENOMEM)?;
        self.valloc_next = aligned_next.add(align_up(len, PAGE_SIZE));
        if self.valloc_next >= USER_VALLOC_END {
            return Err(Errno::ENOMEM.into());
        }
        Ok(aligned_next)
    }
}

// Vm::Drop decrements CoW refcounts and frees intermediate page table pages
// for forked VMs. Only forked VMs are torn down — exec'd VMs' page table
// pages are leaked (~20-40KB/process, same as before this fix).
// Critical for fork+exec performance: without it, the parent's page
// refcounts stay elevated after the forked page table is discarded by exec,
// forcing full page copies on every subsequent CoW fault.
impl Drop for Vm {
    fn drop(&mut self) {
        if self.is_ghost_forked {
            self.page_table.teardown_ghost_pages();
        } else if self.is_forked {
            self.page_table.teardown_forked_pages();
        }
    }
}
