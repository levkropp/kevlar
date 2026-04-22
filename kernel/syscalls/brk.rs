// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::result::Result;
use crate::{process::current_process, syscalls::SyscallHandler};
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    pub fn sys_brk(&mut self, new_heap_end: Option<UserVAddr>) -> Result<isize> {
        let current = current_process();
        let vm_ref = current.vm();
        let (result, heap_end) = {
            let mut vm = vm_ref.as_ref().unwrap().lock_no_irq();
            let result = if let Some(new_heap_end) = new_heap_end {
                vm.expand_heap_to(new_heap_end).ok()
            } else {
                None
            };
            (result, vm.heap_end().value() as isize)
            // vm lock drops here
        };
        // Commit the heap-shrink cleanup OUTSIDE the vm lock. This does the
        // cross-CPU TLB flush + free_pages. Holding the lock across the
        // flush would deadlock a remote CPU's page-fault handler spinning
        // on the same lock with IF=0 (blog 199).
        if let Some(shrink) = result {
            shrink.commit();
        }
        Ok(heap_end)
    }
}
