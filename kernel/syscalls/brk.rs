// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::result::Result;
use crate::{process::current_process, syscalls::SyscallHandler};
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    pub fn sys_brk(&mut self, new_heap_end: Option<UserVAddr>) -> Result<isize> {
        let current = current_process();
        let vm_ref = current.vm();
        let mut vm = vm_ref.as_ref().unwrap().lock_no_irq();
        if let Some(new_heap_end) = new_heap_end {
            // Linux brk() semantics: always return the current program break,
            // never an error. On failure the break is unchanged; the caller
            // detects failure by comparing the returned value to what it asked for.
            let _ = vm.expand_heap_to(new_heap_end);
        }
        Ok(vm.heap_end().value() as isize)
    }
}
