// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use super::{IoVec, IOV_MAX, MAX_READ_WRITE_LEN};
use crate::prelude::*;
use crate::{fs::opened_file::Fd, user_buffer::UserBuffer};
use crate::{process::current_process, syscalls::SyscallHandler};
use core::cmp::min;
use kevlar_platform::address::UserVAddr;

use core::mem::size_of;

impl<'a> SyscallHandler<'a> {
    pub fn sys_writev(&mut self, fd: Fd, iov_base: UserVAddr, iov_count: usize) -> Result<isize> {
        let iov_count = min(iov_count, IOV_MAX);

        current_process().with_file(fd, |opened_file| {
            let mut total_len: usize = 0;
            for i in 0..iov_count {
                let mut iov: IoVec = iov_base.add(i * size_of::<IoVec>()).read()?;

                match total_len.checked_add(iov.len) {
                    Some(len) if len > MAX_READ_WRITE_LEN => {
                        iov.len = MAX_READ_WRITE_LEN - total_len;
                    }
                    None => {
                        iov.len = MAX_READ_WRITE_LEN - total_len;
                    }
                    _ => {}
                }

                if iov.len == 0 {
                    continue;
                }

                total_len += opened_file.write(UserBuffer::from_uaddr(iov.base, iov.len))?;
            }

            Ok(total_len as isize)
        })
    }
}
