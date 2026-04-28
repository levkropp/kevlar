// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use super::MAX_READ_WRITE_LEN;
use crate::prelude::*;
use crate::{fs::opened_file::Fd, user_buffer::UserBuffer};
use crate::{process::current_process, syscalls::SyscallHandler};
use core::cmp::min;
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    pub fn sys_write(&mut self, fd: Fd, uaddr: UserVAddr, len: usize) -> Result<isize> {
        let len = min(len, MAX_READ_WRITE_LEN);

        // Debug: log stderr writes from PID 1 (systemd error messages).
        // Only in debug builds to avoid hot-path overhead in release benchmarks.
        #[cfg(debug_assertions)]
        if fd.as_int() == 2 && current_process().pid().as_i32() == 1 && len > 0 {
            let mut buf = [0u8; 256];
            let copy_len = min(len, buf.len());
            if uaddr.read_bytes(&mut buf[..copy_len]).is_ok() {
                if let Ok(s) = core::str::from_utf8(&buf[..copy_len]) {
                    warn!("systemd stderr: {}", s.trim_end());
                }
            }
        }

        // K33: strace-comm stderr capture (release-safe, opt-in).
        if fd.as_int() == 2 && len > 0 && super::current_process_matches_strace_comm() {
            let mut buf = [0u8; 256];
            let copy_len = min(len, buf.len());
            if uaddr.read_bytes(&mut buf[..copy_len]).is_ok() {
                let s = core::str::from_utf8(&buf[..copy_len]).unwrap_or("<non-utf8>");
                let pid = current_process().pid().as_i32();
                warn!("STRACE-STDERR pid={} ({} bytes): {}", pid, len, s.trim_end());
            }
        }

        current_process().with_file(fd, |opened_file| {
            let written_len = opened_file.write(UserBuffer::from_uaddr(uaddr, len))?;
            Ok(written_len as isize)
        })
    }
}
