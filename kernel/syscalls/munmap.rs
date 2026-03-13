// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
// Own implementation based on Linux man pages.
use crate::prelude::*;
use crate::process::current_process;
use crate::syscalls::SyscallHandler;
use kevlar_platform::{
    address::{PAddr, UserVAddr},
    arch::PAGE_SIZE,
    page_allocator::free_pages,
};
use kevlar_utils::alignment::is_aligned;

impl<'a> SyscallHandler<'a> {
    pub fn sys_munmap(&mut self, addr: UserVAddr, len: usize) -> Result<isize> {
        if !is_aligned(addr.value(), PAGE_SIZE) {
            return Err(Errno::EINVAL.into());
        }

        if len == 0 {
            return Err(Errno::EINVAL.into());
        }

        let len = kevlar_utils::alignment::align_up(len, PAGE_SIZE);

        let current = current_process();
        kevlar_platform::flight_recorder::record(
            kevlar_platform::flight_recorder::kind::MUNMAP,
            current.pid().as_i32() as u32,
            addr.value() as u64,
            len as u64,
        );
        let vm_ref = current.vm();
        // lock_preempt: keeps IF=1 (remote CPUs can ACK TLB shootdown IPIs)
        // but disables preemption (prevents the timer from calling switch() on
        // this CPU while we hold the lock, which would deadlock trying to
        // re-acquire the same SpinMutex).
        let mut vm = vm_ref.as_ref().unwrap().lock_preempt();

        // Remove VMAs in the range (splits at boundaries).
        vm.remove_vma_range(addr, len)?;

        // Walk the page table: clear PTEs and collect physical pages.
        // We do local invlpg per page here, then ONE remote TLB flush IPI
        // for the entire range (instead of one IPI per page), reducing
        // IPI overhead from O(pages) to O(1) per sys_munmap call.
        let num_pages = len / PAGE_SIZE;
        let mut to_free: alloc::vec::Vec<PAddr> = alloc::vec::Vec::new();
        for i in 0..num_pages {
            let page_addr = addr.add(i * PAGE_SIZE);
            if let Some(paddr) = vm.page_table_mut().unmap_user_page(page_addr) {
                // Local TLB invalidation only — no IPI yet.
                vm.page_table().flush_tlb_local(page_addr);
                to_free.push(paddr);
            }
        }

        // One batch remote TLB flush: remote CPUs reload CR3.
        // Must happen BEFORE freeing pages so no CPU can write through a
        // stale TLB entry to a page that has been returned to the allocator.
        if !to_free.is_empty() {
            vm.page_table().flush_tlb_remote();
        }

        // Now safe to free all unmapped physical pages.
        for paddr in to_free {
            free_pages(paddr, 1);
        }

        Ok(0)
    }
}
