// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! recvmsg(2) syscall handler.
//!
//! Supports SCM_RIGHTS (fd passing) over AF_UNIX sockets.
//!
//! Provenance: Own (Linux recvmsg(2), cmsg(3) man pages).
use crate::{
    ctypes::c_int,
    fs::opened_file::{Fd, OpenOptions},
    net::{AncillaryData, UnixSocket, UnixStream},
    prelude::*,
    process::current_process,
    syscalls::SyscallHandler,
    user_buffer::UserBufferMut,
};
use kevlar_platform::address::UserVAddr;
use kevlar_utils::downcast::Downcastable;

// Linux cmsg constants.
const SOL_SOCKET: i32 = 1;
const SCM_RIGHTS: i32 = 1;

/// `struct msghdr` layout (x86_64 / aarch64).
#[repr(C)]
struct MsgHdr {
    msg_name: usize,
    msg_namelen: u32,
    _pad0: u32,
    msg_iov: usize,
    msg_iovlen: usize,
    msg_control: usize,
    msg_controllen: usize,
    msg_flags: c_int,
}

/// `struct cmsghdr` layout.
#[repr(C)]
struct CmsgHdr {
    cmsg_len: usize,
    cmsg_level: c_int,
    cmsg_type: c_int,
}

const CMSG_HDR_SIZE: usize = core::mem::size_of::<CmsgHdr>();

const fn cmsg_align(len: usize) -> usize {
    (len + core::mem::size_of::<usize>() - 1) & !(core::mem::size_of::<usize>() - 1)
}

impl<'a> SyscallHandler<'a> {
    pub fn sys_recvmsg(
        &mut self,
        fd: Fd,
        msg_ptr: UserVAddr,
        _flags: c_int,
    ) -> Result<isize> {
        // Read msghdr from userspace.
        let msghdr = msg_ptr.read::<MsgHdr>()?;

        // Read data into iovecs.
        let opened_file = current_process().get_opened_file_by_fd(fd)?;
        let options = opened_file.options();
        let file = opened_file.as_file()?;

        let mut total = 0usize;
        for i in 0..msghdr.msg_iovlen {
            let iov_ptr = UserVAddr::new_nonnull(
                msghdr.msg_iov + i * core::mem::size_of::<super::IoVec>(),
            )?;
            let iov = iov_ptr.read::<super::IoVec>()?;
            if iov.len == 0 {
                continue;
            }
            let buf = UserBufferMut::from_uaddr(iov.base, iov.len);
            let read_len = file.read(0, buf, &options)?;
            total += read_len;
            if read_len < iov.len {
                break; // short read
            }
        }

        // Process ancillary data: install received fds via SCM_RIGHTS.
        if msghdr.msg_control != 0 && msghdr.msg_controllen > 0 {
            let cmsg_written = self.process_recvmsg_cmsg(
                fd,
                msghdr.msg_control,
                msghdr.msg_controllen,
            )?;

            // Update msg_controllen to reflect how much cmsg data we wrote.
            // MsgHdr layout (repr(C), 64-bit):
            //   msg_name(8) + msg_namelen(4) + pad(4) + msg_iov(8) + msg_iovlen(8)
            //   + msg_control(8) + msg_controllen(8) + msg_flags(4)
            // msg_controllen is at offset 40.
            let controllen_ptr = UserVAddr::new_nonnull(msg_ptr.value() + 40)?;
            controllen_ptr.write::<usize>(&cmsg_written)?;
        } else {
            let controllen_ptr = UserVAddr::new_nonnull(msg_ptr.value() + 40)?;
            controllen_ptr.write::<usize>(&0usize)?;
        }

        // Clear msg_flags (offset 48).
        let flags_ptr = UserVAddr::new_nonnull(msg_ptr.value() + 48)?;
        flags_ptr.write::<c_int>(&0)?;

        Ok(total as isize)
    }

    /// Check for pending ancillary data and write SCM_RIGHTS cmsgs to userspace.
    fn process_recvmsg_cmsg(
        &self,
        fd: Fd,
        control_addr: usize,
        control_len: usize,
    ) -> Result<usize> {
        // Get the UnixStream to check for pending ancillary data.
        let opened_file = current_process().get_opened_file_by_fd(fd)?;
        let file = opened_file.as_file()?;

        // Get the inner UnixStream. In practice, fds are always UnixSocket wrappers.
        let inner_stream: Option<Arc<UnixStream>> =
            if let Some(sock) = file.as_any().downcast_ref::<UnixSocket>() {
                sock.connected_stream()
            } else {
                None
            };

        let stream = match inner_stream {
            Some(ref s) => s,
            None => return Ok(0),
        };

        let mut offset = 0;

        while let Some(ancillary) = stream.recv_ancillary() {
            match ancillary {
                AncillaryData::Rights(files) => {
                    let num_fds = files.len();
                    let data_len = num_fds * core::mem::size_of::<c_int>();
                    let cmsg_len = CMSG_HDR_SIZE + data_len;
                    let aligned_len = cmsg_align(cmsg_len);

                    if offset + aligned_len > control_len {
                        break; // no room for this cmsg
                    }

                    // Write cmsg header.
                    let cmsg = CmsgHdr {
                        cmsg_len,
                        cmsg_level: SOL_SOCKET,
                        cmsg_type: SCM_RIGHTS,
                    };
                    let cmsg_ptr = UserVAddr::new_nonnull(control_addr + offset)?;
                    cmsg_ptr.write::<CmsgHdr>(&cmsg)?;

                    // Install each file in the receiver's fd table and write
                    // the new fd number into the cmsg data area.
                    let data_start = control_addr + offset + CMSG_HDR_SIZE;
                    let install_options = OpenOptions::empty();

                    for (i, opened) in files.into_iter().enumerate() {
                        // Install the Arc<OpenedFile> into the receiver's table.
                        let new_fd = current_process()
                            .opened_files()
                            .lock()
                            .open(
                                opened.path().clone(),
                                install_options,
                            )?;

                        let fd_ptr = UserVAddr::new_nonnull(
                            data_start + i * core::mem::size_of::<c_int>(),
                        )?;
                        fd_ptr.write::<c_int>(&new_fd.as_int())?;
                    }

                    offset += aligned_len;
                }
            }
        }

        Ok(offset)
    }
}
