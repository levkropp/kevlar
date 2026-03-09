// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX getgroups(2) man page).
// Stub — returns 0 (no supplementary groups).
use crate::{prelude::*, syscalls::SyscallHandler};

impl<'a> SyscallHandler<'a> {
    pub fn sys_getgroups(&mut self, _size: usize, _list: usize) -> Result<isize> {
        Ok(0)
    }
}
