// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::fs::path::Path;
use crate::prelude::*;
use crate::process::Process;
use crate::user_buffer::UserCStr;
use crate::{process::current_process, syscalls::SyscallHandler};
use core::mem::size_of;
use kevlar_platform::address::UserVAddr;
use kevlar_vfs::stat::{S_ISUID, S_ISGID};

const ARG_MAX: usize = 512;
const ARG_LEN_MAX: usize = 4096;
const ENV_MAX: usize = 512;
const ENV_LEN_MAX: usize = 4096;

impl<'a> SyscallHandler<'a> {
    pub fn sys_execve(
        &mut self,
        path: &Path,
        argv_uaddr: UserVAddr,
        envp_uaddr: UserVAddr,
    ) -> Result<isize> {
        let current = current_process();
        let root_fs = current.root_fs();
        let executable = root_fs.lock().lookup_path(path, true)?;

        // DAC permission check: require execute permission on the file.
        let stat = executable.inode.stat()?;
        crate::fs::permission::check_access(
            &stat, current.euid(), current.egid(), crate::fs::permission::X_OK,
        )?;

        // S_ISUID: set effective UID to the file's owner.
        // S_ISGID: set effective GID to the file's group.
        let mode = stat.mode.as_u32();
        if mode & S_ISUID != 0 {
            let owner = stat.uid.as_u32();
            current.set_euid(owner);
            current.set_suid(owner);
        }
        if mode & S_ISGID != 0 {
            let group = stat.gid.as_u32();
            current.set_egid(group);
            current.set_sgid(group);
        }

        let mut argv = Vec::new();
        for i in 0..ARG_MAX {
            let ptr = argv_uaddr.add(i * size_of::<usize>());
            match UserVAddr::new(ptr.read::<usize>()?) {
                Some(str_ptr) => argv.push(UserCStr::new(str_ptr, ARG_LEN_MAX)?),
                None => break,
            }
        }

        let mut envp = Vec::new();
        for i in 0..ENV_MAX {
            let ptr = envp_uaddr.add(i * size_of::<usize>());
            match UserVAddr::new(ptr.read::<usize>()?) {
                Some(str_ptr) => envp.push(UserCStr::new(str_ptr, ENV_LEN_MAX)?),
                None => break,
            }
        }

        let argv_slice: Vec<&[u8]> = argv.as_slice().iter().map(|s| s.as_bytes()).collect();
        let envp_slice: Vec<&[u8]> = envp.as_slice().iter().map(|s| s.as_bytes()).collect();
        Process::execve(self.frame, executable, &argv_slice, &envp_slice)?;
        Ok(0)
    }
}
