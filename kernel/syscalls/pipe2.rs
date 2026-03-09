// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use core::mem::size_of;

use alloc::sync::Arc;
use kevlar_platform::address::UserVAddr;

use crate::{
    ctypes::*,
    fs::{
        inode::{FileLike, INode},
        opened_file::{OpenFlags, OpenOptions, PathComponent},
    },
    pipe::Pipe,
    result::Result,
};
use crate::{process::current_process, syscalls::SyscallHandler};
use crate::user_buffer::UserBufWriter;

impl<'a> SyscallHandler<'a> {
    pub fn sys_pipe2(&mut self, fds: UserVAddr, flags: c_int) -> Result<isize> {
        let cloexec = (flags & OpenFlags::O_CLOEXEC.bits()) != 0;
        let nonblock = (flags & OpenFlags::O_NONBLOCK.bits()) != 0;
        let options = OpenOptions::new(nonblock, cloexec);

        let pipe = Pipe::new();
        let read_fd = current_process().opened_files().lock().open(
            PathComponent::new_anonymous(INode::FileLike(pipe.read_end() as Arc<dyn FileLike>)),
            options,
        )?;

        let write_fd = current_process().opened_files().lock().open(
            PathComponent::new_anonymous(INode::FileLike(pipe.write_end() as Arc<dyn FileLike>)),
            options,
        )?;

        let mut fds_writer = UserBufWriter::from_uaddr(fds, 2 * size_of::<c_int>());
        fds_writer.write::<c_int>(read_fd.as_int())?;
        fds_writer.write::<c_int>(write_fd.as_int())?;
        Ok(0)
    }
}
