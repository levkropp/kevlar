// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (Linux sendfile(2) man page).
use crate::fs::opened_file::Fd;
use crate::prelude::*;
use crate::process::current_process;
use crate::syscalls::SyscallHandler;
use crate::user_buffer::{UserBuffer, UserBufferMut};
use core::cmp::min;
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    pub fn sys_sendfile(
        &mut self,
        out_fd: Fd,
        in_fd: Fd,
        offset_ptr: Option<UserVAddr>,
        count: usize,
    ) -> Result<isize> {
        let current = current_process();
        let opened_files = current.opened_files().lock();
        let in_file = opened_files.get(in_fd)?;
        let out_file = opened_files.get(out_fd)?;

        let in_filelike = in_file.as_file()?;
        let in_options = in_file.options();
        let out_filelike = out_file.as_file()?;
        let out_options = out_file.options();

        // Determine read offset: from offset_ptr if provided, else from file position.
        let mut offset = if let Some(ptr) = offset_ptr {
            ptr.read::<i64>()? as usize
        } else {
            in_file.pos()
        };

        let mut total = 0usize;
        let mut buf = [0u8; 4096];
        let mut out_offset = out_file.pos();

        while total < count {
            let chunk = min(count - total, buf.len());
            let n = in_filelike.read(offset, UserBufferMut::from(&mut buf[..chunk]), &in_options)?;
            if n == 0 {
                break;
            }

            let mut written = 0;
            while written < n {
                let w = out_filelike.write(out_offset + written, UserBuffer::from(&buf[written..n]), &out_options)?;
                if w == 0 {
                    break;
                }
                written += w;
            }

            offset += n;
            out_offset += written;
            total += written;
            if written < n {
                break;
            }
        }

        // Update positions.
        out_file.set_pos(out_offset);
        if let Some(ptr) = offset_ptr {
            ptr.write::<i64>(&(offset as i64))?;
        } else {
            in_file.set_pos(offset);
        }

        Ok(total as isize)
    }
}
