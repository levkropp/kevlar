// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! `kvlr_spawn(path, argv, envp, flags)` — atomic fork+exec.
//!
//! Kevlar-private syscall (SYS_KVLR_SPAWN = 500).  Equivalent to a
//! `vfork() + execve()` pair, but executed in one kernel call:
//! constructs the new process directly with the target binary's VM —
//! no intermediate child VM is built and immediately discarded by
//! execve.  Saves the ~10 µs of `Process::fork` work (page table
//! duplication, FpState snapshot, process struct alloc, parent↔child
//! context switch round-trip) that every `fork()+execve()` pair pays
//! for nothing on Linux ABI.
//!
//! Parent blocks until the child has either (a) reached user-mode
//! entry of the new binary or (b) the kernel has failed to set it up.
//! On success returns child PID; on failure returns -errno.
//!
//! See blog 224 for design + benchmark results.

use crate::fs::path::Path;
use crate::process::{current_process, Process};
use crate::result::{Errno, Result};
use crate::syscalls::SyscallHandler;
use crate::user_buffer::UserCStr;
use alloc::vec::Vec;
use core::mem::size_of;
use kevlar_platform::address::UserVAddr;
use kevlar_vfs::stat::{S_ISGID, S_ISUID};

const ARG_MAX: usize = 512;
const ARG_LEN_MAX: usize = 4096;
const ENV_MAX: usize = 512;
const ENV_LEN_MAX: usize = 4096;

impl<'a> SyscallHandler<'a> {
    pub fn sys_kvlr_spawn(
        &mut self,
        path: &Path,
        argv_uaddr: UserVAddr,
        envp_uaddr: UserVAddr,
        flags: u32,
    ) -> Result<isize> {
        if flags != 0 {
            return Err(Errno::EINVAL.into());
        }

        let current = current_process();
        let root_fs = current.root_fs();
        let executable = root_fs.lock().lookup_path(path, true)?;

        // DAC permission check: require execute permission on the file.
        let stat = executable.inode.stat()?;
        crate::fs::permission::check_access(
            &stat,
            current.euid(),
            current.egid(),
            crate::fs::permission::X_OK,
        )?;

        // S_ISUID / S_ISGID handling — mirror sys_execve.  These mutate the
        // PARENT's effective UID/GID before spawning, so the spawned child
        // inherits the elevated credentials via the existing parent→child
        // copy in Process::spawn.
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

        // Copy argv/envp from userspace — same pattern as sys_execve.
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
        let argv_slice: Vec<&[u8]> = argv.iter().map(|s| s.as_bytes()).collect();
        let envp_slice: Vec<&[u8]> = envp.iter().map(|s| s.as_bytes()).collect();

        let child = Process::spawn(current, executable, &argv_slice, &envp_slice)?;
        let child_pid = child.pid().as_i32() as isize;

        // No vfork-style block: the child has its own VM from the start,
        // so the parent can run concurrently — same shape as fork+exec
        // for the parent, just much faster on the kernel side.  The
        // user's wait4() (if any) handles synchronization at the user's
        // discretion.
        Ok(child_pid)
    }
}
