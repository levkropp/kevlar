// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::process::{self, switch};
use crate::result::Result;
use crate::syscalls::SyscallHandler;

impl<'a> SyscallHandler<'a> {
    pub fn sys_sched_yield(&mut self) -> Result<isize> {
        // Fast path: if no other processes are runnable, skip the full
        // context switch (which would enqueue self, pick self, dequeue self).
        if !process::scheduler_is_empty() {
            switch();
        }
        Ok(0)
    }
}
