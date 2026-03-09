// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX readv(2) man page).
use super::IoVec;
use crate::{fs::opened_file::Fd, prelude::*, user_buffer::UserBufferMut};
use crate::{process::current_process, syscalls::SyscallHandler};
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    pub fn sys_readv(
        &mut self,
        fd: Fd,
        iov_uaddr: UserVAddr,
        iovcnt: usize,
    ) -> Result<isize> {
        let opened_file = current_process().get_opened_file_by_fd(fd)?;
        let mut total = 0usize;

        for i in 0..iovcnt {
            let iov_addr = iov_uaddr.add(i * core::mem::size_of::<IoVec>());
            let iov = iov_addr.read::<IoVec>()?;
            if iov.len == 0 {
                continue;
            }

            let buf = UserBufferMut::from_uaddr(iov.base, iov.len);
            let read_len = opened_file.read(buf)?;
            total += read_len;
            if read_len < iov.len {
                break;
            }
        }

        Ok(total as isize)
    }
}
