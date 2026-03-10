// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! inotify_init1(2), inotify_add_watch(2), inotify_rm_watch(2).
//!
//! Provenance: Own (Linux inotify(7) man page).
use crate::{
    ctypes::c_int,
    fs::{
        inode::{FileLike, INode},
        inotify::{InotifyInstance, IN_CLOEXEC, IN_NONBLOCK},
        opened_file::{OpenOptions, PathComponent},
    },
    prelude::*,
    process::current_process,
    syscalls::SyscallHandler,
};
use crate::fs::opened_file::Fd;
use kevlar_platform::address::UserVAddr;

impl<'a> SyscallHandler<'a> {
    pub fn sys_inotify_init1(&mut self, flags: c_int) -> Result<isize> {
        let cloexec = (flags & IN_CLOEXEC) != 0;
        let nonblock = (flags & IN_NONBLOCK) != 0;
        let options = OpenOptions::new(nonblock, cloexec);

        let inst = InotifyInstance::new();
        let fd = current_process().opened_files().lock().open(
            PathComponent::new_anonymous(INode::FileLike(inst as Arc<dyn FileLike>)),
            options,
        )?;
        Ok(fd.as_int() as isize)
    }

    pub fn sys_inotify_add_watch(
        &mut self,
        fd: Fd,
        pathname: UserVAddr,
        mask: u32,
    ) -> Result<isize> {
        let path = super::resolve_path(pathname.value())?;

        let table = current_process().opened_files().lock();
        let file = table.get(fd)?.as_file()?;
        let inotify = (**file)
            .as_any()
            .downcast_ref::<InotifyInstance>()
            .ok_or(Error::new(Errno::EINVAL))?;
        let wd = inotify.add_watch(&path, mask);
        Ok(wd as isize)
    }

    pub fn sys_inotify_rm_watch(&mut self, fd: Fd, wd: c_int) -> Result<isize> {
        let table = current_process().opened_files().lock();
        let file = table.get(fd)?.as_file()?;
        let inotify = (**file)
            .as_any()
            .downcast_ref::<InotifyInstance>()
            .ok_or(Error::new(Errno::EINVAL))?;
        inotify.rm_watch(wd)?;
        Ok(0)
    }
}
