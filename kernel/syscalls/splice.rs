// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (Linux splice(2), tee(2), copy_file_range(2) man pages).
use crate::fs::opened_file::Fd;
use crate::prelude::*;
use crate::process::current_process;
use crate::syscalls::SyscallHandler;
use crate::user_buffer::{UserBuffer, UserBufferMut};
use core::cmp::min;
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    /// splice(2) — transfer data between a pipe and a file descriptor.
    pub fn sys_splice(
        &mut self,
        fd_in: Fd,
        off_in_ptr: Option<UserVAddr>,
        fd_out: Fd,
        off_out_ptr: Option<UserVAddr>,
        len: usize,
        _flags: u32,
    ) -> Result<isize> {
        let current = current_process();
        let opened_files = current.opened_files().lock();
        let in_file = opened_files.get(fd_in)?;
        let out_file = opened_files.get(fd_out)?;

        let in_fl = in_file.as_file()?;
        let in_opts = in_file.options();
        let out_fl = out_file.as_file()?;
        let out_opts = out_file.options();

        let mut in_off = if let Some(ptr) = off_in_ptr {
            ptr.read::<i64>()? as usize
        } else {
            in_file.pos()
        };
        let mut out_off = if let Some(ptr) = off_out_ptr {
            ptr.read::<i64>()? as usize
        } else {
            0 // pipes ignore offset
        };

        let mut total = 0usize;
        let mut buf = [0u8; 4096];

        while total < len {
            let chunk = min(len - total, buf.len());
            let n = in_fl.read(in_off, UserBufferMut::from(&mut buf[..chunk]), &in_opts)?;
            if n == 0 {
                break;
            }

            let mut written = 0;
            while written < n {
                let w = out_fl.write(out_off, UserBuffer::from(&buf[written..n]), &out_opts)?;
                if w == 0 {
                    break;
                }
                written += w;
                out_off += w;
            }

            in_off += n;
            total += written;
            if written < n {
                break;
            }
        }

        if let Some(ptr) = off_in_ptr {
            ptr.write::<i64>(&(in_off as i64))?;
        } else {
            in_file.set_pos(in_file.pos() + total);
        }
        if let Some(ptr) = off_out_ptr {
            ptr.write::<i64>(&(out_off as i64))?;
        }

        Ok(total as isize)
    }

    /// tee(2) — duplicate pipe contents without consuming.
    /// Stub: return EINVAL (both ends must be pipes, and we don't support
    /// non-consuming reads yet).
    pub fn sys_tee(
        &mut self,
        _fd_in: Fd,
        _fd_out: Fd,
        _len: usize,
        _flags: u32,
    ) -> Result<isize> {
        Err(Error::new(Errno::EINVAL))
    }

    /// copy_file_range(2) — in-kernel file-to-file copy.
    pub fn sys_copy_file_range(
        &mut self,
        fd_in: Fd,
        off_in_ptr: Option<UserVAddr>,
        fd_out: Fd,
        off_out_ptr: Option<UserVAddr>,
        len: usize,
        _flags: u32,
    ) -> Result<isize> {
        let current = current_process();
        let opened_files = current.opened_files().lock();
        let in_file = opened_files.get(fd_in)?;
        let out_file = opened_files.get(fd_out)?;

        let in_fl = in_file.as_file()?;
        let in_opts = in_file.options();
        let out_fl = out_file.as_file()?;
        let out_opts = out_file.options();

        let mut in_off = if let Some(ptr) = off_in_ptr {
            ptr.read::<i64>()? as usize
        } else {
            in_file.pos()
        };
        let mut out_off = if let Some(ptr) = off_out_ptr {
            ptr.read::<i64>()? as usize
        } else {
            out_file.pos()
        };

        let mut total = 0usize;
        let mut buf = [0u8; 4096];

        while total < len {
            let chunk = min(len - total, buf.len());
            let n = in_fl.read(in_off, UserBufferMut::from(&mut buf[..chunk]), &in_opts)?;
            if n == 0 {
                break;
            }

            let mut written = 0;
            while written < n {
                let w = out_fl.write(out_off, UserBuffer::from(&buf[written..n]), &out_opts)?;
                if w == 0 {
                    break;
                }
                written += w;
                out_off += w;
            }

            in_off += n;
            total += written;
            if written < n {
                break;
            }
        }

        if let Some(ptr) = off_in_ptr {
            ptr.write::<i64>(&(in_off as i64))?;
        } else {
            in_file.set_pos(in_file.pos() + total);
        }
        if let Some(ptr) = off_out_ptr {
            ptr.write::<i64>(&(out_off as i64))?;
        } else {
            out_file.set_pos(out_file.pos() + total);
        }

        Ok(total as isize)
    }
}
