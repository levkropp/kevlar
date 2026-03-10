// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (Linux preadv(2), pwritev(2) man pages).
use super::IoVec;
use crate::fs::opened_file::Fd;
use crate::prelude::*;
use crate::process::current_process;
use crate::syscalls::SyscallHandler;
use crate::user_buffer::{UserBuffer, UserBufferMut};
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    pub fn sys_preadv(
        &mut self,
        fd: Fd,
        iov_uaddr: UserVAddr,
        iovcnt: usize,
        offset: usize,
    ) -> Result<isize> {
        let opened_file = current_process().get_opened_file_by_fd(fd)?;
        let file = opened_file.as_file()?;
        let options = opened_file.options();
        let mut total = 0usize;
        let mut cur_offset = offset;

        for i in 0..iovcnt {
            let iov_addr = iov_uaddr.add(i * core::mem::size_of::<IoVec>());
            let iov = iov_addr.read::<IoVec>()?;
            if iov.len == 0 {
                continue;
            }

            let buf = UserBufferMut::from_uaddr(iov.base, iov.len);
            let read_len = file.read(cur_offset, buf, &options)?;
            total += read_len;
            cur_offset += read_len;
            if read_len < iov.len {
                break;
            }
        }

        Ok(total as isize)
    }

    pub fn sys_pwritev(
        &mut self,
        fd: Fd,
        iov_uaddr: UserVAddr,
        iovcnt: usize,
        offset: usize,
    ) -> Result<isize> {
        let opened_file = current_process().get_opened_file_by_fd(fd)?;
        let file = opened_file.as_file()?;
        let options = opened_file.options();
        let mut total = 0usize;
        let mut cur_offset = offset;

        for i in 0..iovcnt {
            let iov_addr = iov_uaddr.add(i * core::mem::size_of::<IoVec>());
            let iov = iov_addr.read::<IoVec>()?;
            if iov.len == 0 {
                continue;
            }

            let buf = UserBuffer::from_uaddr(iov.base, iov.len);
            let written_len = file.write(cur_offset, buf, &options)?;
            total += written_len;
            cur_offset += written_len;
            if written_len < iov.len {
                break;
            }
        }

        Ok(total as isize)
    }
}
