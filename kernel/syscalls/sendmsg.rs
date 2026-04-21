// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! sendmsg(2) syscall handler.
//!
//! Supports SCM_RIGHTS (fd passing) over AF_UNIX sockets.
//!
//! Provenance: Own (Linux sendmsg(2), cmsg(3) man pages).
use crate::{
    ctypes::c_int,
    fs::opened_file::Fd,
    net::{AncillaryData, UnixSocket, UnixStream},
    prelude::*,
    process::current_process,
    syscalls::SyscallHandler,
    user_buffer::UserBuffer,
};
use kevlar_platform::address::UserVAddr;


// Linux cmsg constants.
const SOL_SOCKET: i32 = 1;
const SCM_RIGHTS: i32 = 1;

/// `struct msghdr` layout (x86_64 / aarch64).
#[repr(C)]
struct MsgHdr {
    msg_name: usize,       // optional address
    msg_namelen: u32,
    _pad0: u32,            // alignment padding on 64-bit
    msg_iov: usize,        // pointer to iovec array
    msg_iovlen: usize,     // number of iovecs
    msg_control: usize,    // ancillary data
    msg_controllen: usize, // ancillary data length
    msg_flags: c_int,
}

/// `struct cmsghdr` layout.
#[repr(C)]
struct CmsgHdr {
    cmsg_len: usize,
    cmsg_level: c_int,
    cmsg_type: c_int,
}

/// Size of cmsghdr header (aligned).
const CMSG_HDR_SIZE: usize = core::mem::size_of::<CmsgHdr>();

/// CMSG_ALIGN: round up to pointer alignment.
const fn cmsg_align(len: usize) -> usize {
    (len + core::mem::size_of::<usize>() - 1) & !(core::mem::size_of::<usize>() - 1)
}

impl<'a> SyscallHandler<'a> {
    pub fn sys_sendmsg(
        &mut self,
        fd: Fd,
        msg_ptr: UserVAddr,
        _flags: c_int,
    ) -> Result<isize> {
        // Read msghdr from userspace.
        let msghdr = msg_ptr.read::<MsgHdr>()?;

        // Process ancillary data (SCM_RIGHTS).
        if msghdr.msg_control != 0 && msghdr.msg_controllen > 0 {
            self.process_sendmsg_cmsg(fd, msghdr.msg_control, msghdr.msg_controllen)?;
        }

        // Gather data from iovecs and write to the socket.
        let opened_file = current_process().get_opened_file_by_fd(fd)?;
        let options = opened_file.options();
        let file = opened_file.as_file()?;

        // Collect iovecs and their total length so we can decide whether
        // to consolidate into a single atomic sendto (match Linux's
        // atomic-writev semantics, which D-Bus and ICE rely on for
        // length-prefixed framing — see blog 201 + kernel/syscalls/writev.rs).
        const SENDMSG_ATOMIC_LIMIT: usize = 64 * 1024;
        let mut iovs: alloc::vec::Vec<super::IoVec> = alloc::vec::Vec::new();
        let mut total_len: usize = 0;
        for i in 0..msghdr.msg_iovlen {
            let iov_ptr = UserVAddr::new_nonnull(
                msghdr.msg_iov + i * core::mem::size_of::<super::IoVec>(),
            )?;
            let iov = iov_ptr.read::<super::IoVec>()?;
            if iov.len > 0 {
                total_len = total_len.saturating_add(iov.len);
                iovs.push(iov);
            }
        }

        if total_len == 0 {
            return Ok(0);
        }

        // Atomic path: consolidate all iovecs into one kernel buffer and
        // issue a single sendto(). Prevents interleaved writes from
        // concurrent senders on the same fd from scrambling the stream.
        if total_len <= SENDMSG_ATOMIC_LIMIT {
            let mut buf = alloc::vec![0u8; total_len];
            let mut off: usize = 0;
            for iov in &iovs {
                iov.base.read_bytes(&mut buf[off..off + iov.len])?;
                off += iov.len;
            }
            let written = file.sendto(
                UserBuffer::from(buf.as_slice()),
                None,
                &options,
            )?;
            return Ok(written as isize);
        }

        // Fallback: per-iovec sendto for huge messages. Not atomic.
        let mut total = 0usize;
        for iov in &iovs {
            let buf = UserBuffer::from_uaddr(iov.base, iov.len);
            let written = file.sendto(buf, None, &options)?;
            total += written;
        }

        Ok(total as isize)
    }

    /// Parse cmsg headers and handle SCM_RIGHTS.
    fn process_sendmsg_cmsg(
        &self,
        fd: Fd,
        control_addr: usize,
        control_len: usize,
    ) -> Result<()> {
        let mut offset = 0;

        while offset + CMSG_HDR_SIZE <= control_len {
            let cmsg_ptr = UserVAddr::new_nonnull(control_addr + offset)?;
            let cmsg = cmsg_ptr.read::<CmsgHdr>()?;

            if cmsg.cmsg_len < CMSG_HDR_SIZE {
                break;
            }

            if cmsg.cmsg_level == SOL_SOCKET && cmsg.cmsg_type == SCM_RIGHTS {
                let data_len = cmsg.cmsg_len - CMSG_HDR_SIZE;
                let num_fds = data_len / core::mem::size_of::<c_int>();

                if num_fds > 0 {
                    let mut files = Vec::new();
                    let data_ptr = UserVAddr::new_nonnull(control_addr + offset + CMSG_HDR_SIZE)?;

                    for j in 0..num_fds {
                        let fd_ptr = UserVAddr::new_nonnull(
                            data_ptr.value() + j * core::mem::size_of::<c_int>(),
                        )?;
                        let src_fd = Fd::new(fd_ptr.read::<c_int>()?);
                        let opened = current_process().get_opened_file_by_fd(src_fd)?;
                        files.push(opened);
                    }

                    // Find the UnixStream to attach ancillary data.
                    let opened_file = current_process().get_opened_file_by_fd(fd)?;
                    let file = opened_file.as_file()?;
                    if let Some(stream) = (**file).as_any().downcast_ref::<UnixStream>() {
                        stream.send_ancillary(AncillaryData::Rights(files));
                    } else if let Some(sock) = (**file).as_any().downcast_ref::<UnixSocket>() {
                        if let Some(stream) = sock.connected_stream() {
                            stream.send_ancillary(AncillaryData::Rights(files));
                        }
                    }
                }
            }

            offset += cmsg_align(cmsg.cmsg_len);
        }

        Ok(())
    }
}
