// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use core::mem::size_of;

use kevlar_platform::address::UserVAddr;

use crate::{
    ctypes::{c_int, c_nfds, c_short},
    fs::{inode::PollStatus, opened_file::Fd},
    poll::POLL_WAIT_QUEUE,
    prelude::*,
    process::current_process,
    syscalls::SyscallHandler,
    timer::read_monotonic_clock,
    user_buffer::UserBuffer,
};

use crate::user_buffer::UserBufReader;

impl<'a> SyscallHandler<'a> {
    pub fn sys_poll(&mut self, fds: UserVAddr, nfds: c_nfds, timeout: c_int) -> Result<isize> {
        let started_at = read_monotonic_clock();
        POLL_WAIT_QUEUE.sleep_signalable_until(|| {
            // Check the statuses of all specified files one by one.
            let mut ready_fds = 0;
            let fds_len = (nfds as usize) * (size_of::<Fd>() + 2 * size_of::<c_short>());
            let mut reader = UserBufReader::from(UserBuffer::from_uaddr(fds, fds_len));
            for _ in 0..nfds {
                let fd = reader.read::<Fd>()?;
                let events = bitflags_from_user!(PollStatus, reader.read::<c_short>()?)?;

                let revents = if fd.as_int() < 0 || events.is_empty() {
                    0
                } else {
                    // Look up the opened file.  POSIX says an invalid fd in
                    // the pollfd array must cause POLLNVAL in *revents* for
                    // that fd — NOT a failure of the whole poll call.
                    // Previously we returned `?` which propagated EBADF and
                    // crashed every poll loop that had one stale fd (e.g.
                    // xfce4-session after a child dbus-daemon closed).
                    match current_process().opened_files_no_irq().get(fd) {
                        Err(_) => {
                            ready_fds += 1;
                            PollStatus::POLLNVAL.bits()
                        }
                        Ok(file) => {
                            let status = file.poll()?;
                            // POLLHUP, POLLERR, POLLNVAL are always reported
                            // regardless of requested events (POSIX).
                            let always = status & (PollStatus::POLLHUP | PollStatus::POLLERR);
                            let revents = (events & status) | always;
                            if !revents.is_empty() {
                                ready_fds += 1;
                            }
                            revents.bits()
                        }
                    }
                };

                // Update revents.
                fds.add(reader.pos()).write::<c_short>(&revents)?;

                // Skip revents in the reader.
                reader.skip(size_of::<c_short>())?;
            }

            if ready_fds > 0 {
                return Ok(Some(ready_fds));
            }

            // No fds ready — check timeout.
            if timeout >= 0 && started_at.elapsed_msecs() >= (timeout as usize) {
                return Ok(Some(0));
            }

            // Sleep until any changes in files or sockets occur...
            Ok(None)
        })
    }
}
