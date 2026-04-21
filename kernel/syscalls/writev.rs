// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use super::{IoVec, IOV_MAX, MAX_READ_WRITE_LEN};
use crate::prelude::*;
use crate::{fs::opened_file::Fd, user_buffer::UserBuffer};
use crate::{process::current_process, syscalls::SyscallHandler};
use core::cmp::min;
use kevlar_platform::address::UserVAddr;

use core::mem::size_of;

/// Consolidate-threshold: if the total iovec length is at most this, we
/// gather all iovecs into a single kernel buffer and do ONE `write()`
/// call. That makes writev atomic w.r.t. other writers on the same fd —
/// matching Linux semantics for small AF_UNIX / pipe writes. Above this
/// threshold we fall back to per-iovec writes; huge writev calls
/// (GB-scale) would blow the kernel heap if consolidated, and for those
/// atomicity isn't reasonably expected anyway.
const WRITEV_ATOMIC_LIMIT: usize = 64 * 1024;

impl<'a> SyscallHandler<'a> {
    pub fn sys_writev(&mut self, fd: Fd, iov_base: UserVAddr, iov_count: usize) -> Result<isize> {
        let iov_count = min(iov_count, IOV_MAX);

        // First pass: read all iovecs and compute total length, capping at
        // MAX_READ_WRITE_LEN. We'll consolidate below if it fits the atomic
        // limit; otherwise fall back to the old per-iovec loop.
        let mut iovs: alloc::vec::Vec<IoVec> = alloc::vec::Vec::with_capacity(iov_count);
        let mut total_len: usize = 0;
        for i in 0..iov_count {
            let mut iov: IoVec = iov_base.add(i * size_of::<IoVec>()).read()?;
            match total_len.checked_add(iov.len) {
                Some(len) if len > MAX_READ_WRITE_LEN => {
                    iov.len = MAX_READ_WRITE_LEN - total_len;
                }
                None => {
                    iov.len = MAX_READ_WRITE_LEN - total_len;
                }
                _ => {}
            }
            total_len += iov.len;
            if iov.len > 0 {
                iovs.push(iov);
            }
            if total_len >= MAX_READ_WRITE_LEN {
                break;
            }
        }

        if total_len == 0 {
            return Ok(0);
        }

        current_process().with_file(fd, |opened_file| {
            // Atomic path: consolidate all iovecs into one kernel buffer and
            // issue a single write(). This prevents interleaved writes from
            // concurrent writers on the same fd from corrupting the stream,
            // which matters for length-prefixed protocols like D-Bus and
            // ICE/XSM. See blog 201 for the reproducer.
            if total_len <= WRITEV_ATOMIC_LIMIT {
                let mut buf = alloc::vec![0u8; total_len];
                let mut off: usize = 0;
                for iov in &iovs {
                    iov.base.read_bytes(&mut buf[off..off + iov.len])?;
                    off += iov.len;
                }
                let written = opened_file.write(UserBuffer::from(buf.as_slice()))?;
                return Ok(written as isize);
            }

            // Fallback path (huge writev): per-iovec writes, non-atomic.
            let mut written_total: usize = 0;
            for iov in &iovs {
                written_total += opened_file.write(
                    UserBuffer::from_uaddr(iov.base, iov.len),
                )?;
            }
            Ok(written_total as isize)
        })
    }
}
