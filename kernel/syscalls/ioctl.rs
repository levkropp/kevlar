// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::result::Result;
use crate::syscalls::SyscallHandler;
use crate::{fs::opened_file::Fd, process::current_process};

impl<'a> SyscallHandler<'a> {
    pub fn sys_ioctl(&mut self, fd: Fd, cmd: usize, arg: usize) -> Result<isize> {
        // Network interface ioctls (SIOCGIF*, SIOCSIF*, etc.) operate on the
        // global smoltcp interface, not a specific file descriptor.
        if (cmd & 0xFF00) == 0x8900 {
            return self.sys_net_ioctl(cmd, arg);
        }

        // FIOCLEX/FIONCLEX — set/clear FD_CLOEXEC (equivalent to fcntl F_SETFD).
        const FIONCLEX: usize = 0x5450;
        const FIOCLEX: usize = 0x5451;
        if cmd == FIOCLEX || cmd == FIONCLEX {
            current_process().opened_files_no_irq().set_cloexec(fd, cmd == FIOCLEX)?;
            return Ok(0);
        }

        let opened_file = current_process().get_opened_file_by_fd(fd)?;

        // FIONBIO — some networking code uses this instead of fcntl(F_SETFL, O_NONBLOCK).
        if cmd == 0x5421 {
            let val = kevlar_platform::address::UserVAddr::new_nonnull(arg)?
                .read::<crate::ctypes::c_int>()?;
            opened_file.set_nonblock(val != 0);
            return Ok(0);
        }

        opened_file.ioctl(cmd, arg)
    }
}
