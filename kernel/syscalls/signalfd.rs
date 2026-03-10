// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! signalfd4(2) syscall handler.
//!
//! Provenance: Own (Linux signalfd(2) man page).
use crate::{
    ctypes::c_int,
    fs::{
        inode::{FileLike, INode},
        opened_file::{OpenOptions, PathComponent},
        signalfd::{SFD_CLOEXEC, SFD_NONBLOCK, SignalFd},
    },
    prelude::*,
    process::current_process,
    process::signal::SigSet,
    syscalls::SyscallHandler,
};
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    /// `signalfd4(fd, mask, flags)` — create or update a signal fd.
    ///
    /// If fd == -1, creates a new signalfd. Otherwise updates an existing one
    /// (updating is not yet supported — returns EINVAL for non-(-1) fd).
    pub fn sys_signalfd4(
        &mut self,
        fd: c_int,
        mask_ptr: UserVAddr,
        _sigsetsize: usize,
        flags: c_int,
    ) -> Result<isize> {
        if fd != -1 {
            // Updating an existing signalfd is not yet supported.
            return Err(Errno::EINVAL.into());
        }

        // Read the signal mask from userspace (8 bytes = sigset_t on x86_64).
        let mask_bytes = mask_ptr.read::<[u8; 8]>()?;
        let mask = SigSet::from_bytes(&mask_bytes);

        let cloexec = (flags & SFD_CLOEXEC) != 0;
        let nonblock = (flags & SFD_NONBLOCK) != 0;
        let options = OpenOptions::new(nonblock, cloexec);

        let sfd = SignalFd::new(mask.bits());
        let new_fd = current_process().opened_files().lock().open(
            PathComponent::new_anonymous(INode::FileLike(sfd as Arc<dyn FileLike>)),
            options,
        )?;
        Ok(new_fd.as_int() as isize)
    }
}
