// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX getgroups(2) man page).
use crate::{prelude::*, process::current_process, syscalls::SyscallHandler};
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    pub fn sys_getgroups(&mut self, size: usize, list: usize) -> Result<isize> {
        let groups = current_process().groups();
        let ngroups = groups.len();

        if size == 0 {
            // Return the number of supplementary groups.
            return Ok(ngroups as isize);
        }
        if ngroups > size {
            return Err(Errno::EINVAL.into());
        }

        if ngroups > 0 {
            let ptr = UserVAddr::new_nonnull(list)?;
            for (i, &gid) in groups.iter().enumerate() {
                ptr.add(i * 4).write::<u32>(&gid)?;
            }
        }
        Ok(ngroups as isize)
    }
}
