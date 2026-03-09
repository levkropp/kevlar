// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use kevlar_platform::address::UserVAddr;

use crate::debug;
use crate::result::{Errno, Result};
use crate::syscalls::SyscallHandler;
use crate::{ctypes::*, process::current_process};

use crate::user_buffer::UserBufWriter;

impl<'a> SyscallHandler<'a> {
    pub fn sys_getcwd(&mut self, buf: UserVAddr, len: c_size) -> Result<isize> {
        let cwd = current_process()
            .root_fs()
            .lock()
            .cwd_path()
            .resolve_absolute_path();

        if (len as usize) < cwd.as_str().as_bytes().len() {
            return Err(Errno::ERANGE.into());
        }

        debug::usercopy::set_context("sys_getcwd");
        let mut writer = UserBufWriter::from_uaddr(buf, len as usize);
        writer.write_bytes(cwd.as_str().as_bytes())?;
        writer.write(0u8)?;
        debug::usercopy::clear_context();
        Ok(buf.as_isize())
    }
}
