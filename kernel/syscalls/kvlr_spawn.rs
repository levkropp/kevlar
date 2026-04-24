// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! `kvlr_spawn(path, argv, envp, flags, file_actions, attr)` — atomic
//! fork+exec with optional posix_spawn-style file actions and attrs.
//!
//! Kevlar-private syscall (SYS_KVLR_SPAWN = 500).
//!
//! ## v1 (flags = 0)
//!
//! Four args: path, argv, envp, flags.  a5 and a6 ignored.  Child
//! inherits parent's file descriptor table (with CLOEXEC applied),
//! credentials, cgroup, namespace, umask.  Fresh SIG_DFL signal handlers.
//!
//! ## v2 (flags & KVLR_SPAWN_F_EXTENDED)
//!
//! a5 = pointer to KvlrSpawnFileActions (or NULL), a6 = pointer to
//! KvlrSpawnAttr (or NULL).  Applied atomically in the kernel between
//! VM setup and child enqueue — the same conceptual slot that
//! `vfork()+execve()` would use them.  Enough surface for musl's
//! posix_spawn() to route through this syscall safely:
//!
//! - **File actions**: OPEN (path + oflag + mode → fd), CLOSE (fd),
//!   DUP2 (src fd → dst fd).  Applied in order on the child's
//!   freshly-cloned fd table.  Covers stdin/stdout/stderr redirection
//!   (the 99% case — Python subprocess, every shell pipeline) plus
//!   programmatic open/close.
//! - **Attrs**: SETSIGMASK, SETSIGDEF, SETPGROUP, SETSID, RESETIDS.
//!   Applied to the child Process before it's scheduled.
//!
//! POSIX correctness goal: every posix_spawn(3) feature that musl
//! implements via vfork+execve must have a semantic-equivalent
//! translation into KvlrSpawnFileAction / KvlrSpawnAttr, so routing
//! posix_spawn through this syscall changes timing but never
//! observable behaviour.  See blog 227.

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
const FILE_ACTIONS_MAX: usize = 64;
const FA_PATH_LEN_MAX: usize = 4096;

/// `flags` bit: a5 = file_actions ptr, a6 = attr ptr.  Without this bit
/// the syscall ignores a5/a6 and behaves as v1 — existing v1 callers
/// (bench.c pre-blog 227) that pass flags=0 stay on the v1 path.
pub const KVLR_SPAWN_F_EXTENDED: u32 = 1 << 0;

// File action ops.
pub const KVLR_SPAWN_FA_CLOSE: u32 = 1;
pub const KVLR_SPAWN_FA_OPEN: u32 = 2;
pub const KVLR_SPAWN_FA_DUP2: u32 = 3;

// Attr flag bits.
pub const KVLR_SPAWN_SETSIGMASK: u32 = 1 << 0;
pub const KVLR_SPAWN_SETSIGDEF: u32 = 1 << 1;
pub const KVLR_SPAWN_SETPGROUP: u32 = 1 << 2;
pub const KVLR_SPAWN_SETSID: u32 = 1 << 3;
pub const KVLR_SPAWN_RESETIDS: u32 = 1 << 4;

/// Wire format for one posix_spawn file action.  `path` is a userspace
/// pointer (only meaningful for OPEN); ignored otherwise.
#[repr(C)]
#[derive(Clone, Copy)]
struct WireFileAction {
    op: u32,
    fd: i32,
    newfd: i32,
    oflag: i32,
    mode: u32,
    _pad: u32,
    path: usize, // UserVAddr as raw usize (0 = none)
}

/// Wire format header for the file_actions array.  Caller allocates
/// `sizeof(header) + count * sizeof(WireFileAction)`.
#[repr(C)]
#[derive(Clone, Copy)]
struct WireFileActionsHeader {
    count: u32,
    _pad: u32,
}

/// Wire format for spawn attrs.
#[repr(C)]
#[derive(Clone, Copy)]
struct WireAttr {
    flags: u32,
    pgid: i32,
    sigmask: u64,
    sigdefault: u64,
}

/// Kernel-side parsed representation of one file action.  Strings are
/// already copied from userspace and owned here.
#[derive(Clone)]
pub enum SpawnFileAction {
    Close(i32),
    Open { fd: i32, path: alloc::string::String, oflag: i32, mode: u32 },
    Dup2 { src: i32, dst: i32 },
}

/// Kernel-side parsed attrs.
#[derive(Clone, Copy, Default)]
pub struct SpawnAttr {
    pub flags: u32,
    pub pgid: i32,
    pub sigmask: u64,
    pub sigdefault: u64,
}

fn copy_file_actions(actions_uaddr: usize) -> Result<Vec<SpawnFileAction>> {
    if actions_uaddr == 0 {
        return Ok(Vec::new());
    }
    let hdr_uva = UserVAddr::new_nonnull(actions_uaddr)?;
    let hdr: WireFileActionsHeader = hdr_uva.read()?;
    let count = hdr.count as usize;
    if count > FILE_ACTIONS_MAX {
        return Err(Errno::E2BIG.into());
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let entry_off = size_of::<WireFileActionsHeader>() + i * size_of::<WireFileAction>();
        let entry_uva = hdr_uva.add(entry_off);
        let wa: WireFileAction = entry_uva.read()?;
        let action = match wa.op {
            KVLR_SPAWN_FA_CLOSE => SpawnFileAction::Close(wa.fd),
            KVLR_SPAWN_FA_OPEN => {
                let path_uva = UserVAddr::new_nonnull(wa.path)?;
                let pstr = UserCStr::new(path_uva, FA_PATH_LEN_MAX)?;
                let s = core::str::from_utf8(pstr.as_bytes())
                    .map_err(|_| Errno::EINVAL)?
                    .into();
                SpawnFileAction::Open { fd: wa.fd, path: s, oflag: wa.oflag, mode: wa.mode }
            }
            KVLR_SPAWN_FA_DUP2 => SpawnFileAction::Dup2 { src: wa.fd, dst: wa.newfd },
            _ => return Err(Errno::EINVAL.into()),
        };
        out.push(action);
    }
    Ok(out)
}

fn copy_attr(attr_uaddr: usize) -> Result<Option<SpawnAttr>> {
    if attr_uaddr == 0 {
        return Ok(None);
    }
    let uva = UserVAddr::new_nonnull(attr_uaddr)?;
    let wa: WireAttr = uva.read()?;
    Ok(Some(SpawnAttr {
        flags: wa.flags,
        pgid: wa.pgid,
        sigmask: wa.sigmask,
        sigdefault: wa.sigdefault,
    }))
}

impl<'a> SyscallHandler<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn sys_kvlr_spawn(
        &mut self,
        path: &Path,
        argv_uaddr: UserVAddr,
        envp_uaddr: UserVAddr,
        flags: u32,
        file_actions_uaddr: usize,
        attr_uaddr: usize,
    ) -> Result<isize> {
        // Unrecognised flag bits → EINVAL.  Only KVLR_SPAWN_F_EXTENDED
        // defined today.
        if flags & !KVLR_SPAWN_F_EXTENDED != 0 {
            return Err(Errno::EINVAL.into());
        }
        let extended = (flags & KVLR_SPAWN_F_EXTENDED) != 0;
        let (file_actions, attr) = if extended {
            (copy_file_actions(file_actions_uaddr)?, copy_attr(attr_uaddr)?)
        } else {
            (Vec::new(), None)
        };

        let current = current_process();
        let root_fs = current.root_fs();
        let executable = root_fs.lock().lookup_path(path, true)?;

        // DAC execute check + SUID/SGID handling.  Mirrors sys_execve.
        let stat = executable.inode.stat()?;
        crate::fs::permission::check_access(
            &stat,
            current.euid(),
            current.egid(),
            crate::fs::permission::X_OK,
        )?;
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

        let child = Process::spawn(
            current,
            executable,
            &argv_slice,
            &envp_slice,
            &file_actions,
            attr.as_ref(),
        )?;
        Ok(child.pid().as_i32() as isize)
    }
}
