// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! memfd_create(2) — anonymous memory file.
use crate::{
    fs::{
        opened_file::PathComponent,
        tmpfs::TmpFs,
    },
    prelude::*,
    process::current_process,
    syscalls::SyscallHandler,
    user_buffer::UserCStr,
};
use kevlar_platform::address::UserVAddr;
use kevlar_vfs::{
    file_system::FileSystem,
    inode::OpenOptions,
    stat::{FileMode, GId, UId},
};

impl<'a> SyscallHandler<'a> {
    pub fn sys_memfd_create(&mut self, name_ptr: UserVAddr, _flags: u32) -> Result<isize> {
        let name = UserCStr::new(name_ptr, 256)?;

        // Create a temporary file in a fresh tmpfs.
        let tmpfs = TmpFs::new();
        let root_dir = tmpfs.root_dir()?;
        let current = current_process();
        let inode = root_dir.create_file(name.as_str(), FileMode::new(0o600), UId::new(current.euid()), GId::new(current.egid()))?;

        let path_component = Arc::new(PathComponent {
            parent_dir: None,
            name: alloc::format!("memfd:{}", name.as_str()),
            inode,
        });

        let fd = current_process()
            .opened_files()
            .lock()
            .open(path_component, OpenOptions::empty())?;

        Ok(fd.as_int() as isize)
    }
}
