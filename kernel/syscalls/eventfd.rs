// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! eventfd2(2) syscall handler.
//!
//! Provenance: Own (Linux eventfd(2) man page).
use crate::{
    ctypes::c_int,
    fs::{
        eventfd::{EFD_CLOEXEC, EFD_NONBLOCK, EFD_SEMAPHORE, EventFd},
        inode::{FileLike, INode},
        opened_file::{OpenOptions, PathComponent},
    },
    prelude::*,
    process::current_process,
    syscalls::SyscallHandler,
};

impl<'a> SyscallHandler<'a> {
    /// `eventfd2(initval, flags)` — create an event notification fd.
    pub fn sys_eventfd2(&mut self, initval: u32, flags: c_int) -> Result<isize> {
        let cloexec = (flags & EFD_CLOEXEC) != 0;
        let nonblock = (flags & EFD_NONBLOCK) != 0;
        let semaphore = (flags & EFD_SEMAPHORE) != 0;
        let options = OpenOptions::new(nonblock, cloexec);

        let efd = EventFd::new(initval, semaphore);
        let fd = current_process().opened_files().lock().open(
            PathComponent::new_anonymous(INode::FileLike(efd as Arc<dyn FileLike>)),
            options,
        )?;
        Ok(fd.as_int() as isize)
    }
}
