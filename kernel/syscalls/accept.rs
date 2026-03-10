// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use kevlar_platform::address::UserVAddr;

use crate::{
    ctypes::c_int,
    fs::opened_file::{Fd, OpenOptions, PathComponent},
    net::socket::write_sockaddr,
    prelude::*,
    process::current_process,
    syscalls::SyscallHandler,
};

const SOCK_CLOEXEC: c_int = 0o2000000;
const SOCK_NONBLOCK: c_int = 0o4000;

impl<'a> SyscallHandler<'a> {
    pub fn sys_accept(
        &mut self,
        fd: Fd,
        sockaddr: Option<UserVAddr>,
        socklen: Option<UserVAddr>,
    ) -> Result<isize> {
        self.sys_accept4(fd, sockaddr, socklen, 0)
    }

    pub fn sys_accept4(
        &mut self,
        fd: Fd,
        sockaddr: Option<UserVAddr>,
        socklen: Option<UserVAddr>,
        flags: c_int,
    ) -> Result<isize> {
        let opened_file = current_process().get_opened_file_by_fd(fd)?;
        let (sock, accepted_sockaddr) = opened_file.accept()?;

        let options = OpenOptions {
            nonblock: (flags & SOCK_NONBLOCK) != 0,
            close_on_exec: (flags & SOCK_CLOEXEC) != 0,
            append: false,
        };
        let fd = current_process()
            .opened_files()
            .lock()
            .open(PathComponent::new_anonymous(sock.into()), options)?;
        write_sockaddr(&accepted_sockaddr, sockaddr, socklen)?;
        Ok(fd.as_usize() as isize)
    }
}
