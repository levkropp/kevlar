// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! /proc/self symlink and /proc/[pid]/ per-process directories.
use core::fmt;

use alloc::string::ToString;
use alloc::sync::Arc;

use kevlar_vfs::{
    inode::{DirEntry, Directory, FileLike, FileType, INode, INodeNo, OpenOptions, Symlink},
    path::PathBuf,
    result::{Errno, Error, Result},
    stat::{FileMode, Stat, S_IFLNK, S_IFDIR, S_IFREG},
    user_buffer::{UserBufWriter, UserBufferMut},
};

use crate::process::{current_process, Process, PId};

// ── /proc/self → /proc/<pid> symlink ────────────────────────────────

pub struct ProcSelfSymlink;

impl fmt::Debug for ProcSelfSymlink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcSelfSymlink").finish()
    }
}

impl Symlink for ProcSelfSymlink {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFLNK | 0o777),
            ..Stat::zeroed()
        })
    }

    fn linked_to(&self) -> Result<PathBuf> {
        let pid = current_process().pid().as_i32();
        Ok(PathBuf::from(alloc::format!("/proc/{}", pid)))
    }
}

// ── /proc/<pid>/ directory ──────────────────────────────────────────

/// A dynamic directory that generates per-process files on the fly.
pub struct ProcPidDir {
    pid: PId,
}

impl ProcPidDir {
    pub fn new(pid: PId) -> Arc<ProcPidDir> {
        Arc::new(ProcPidDir { pid })
    }
}

impl fmt::Debug for ProcPidDir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcPidDir").field("pid", &self.pid).finish()
    }
}

impl Directory for ProcPidDir {
    fn lookup(&self, name: &str) -> Result<INode> {
        match name {
            "stat" => Ok(INode::FileLike(
                Arc::new(ProcPidStat { pid: self.pid }) as Arc<dyn FileLike>
            )),
            "status" => Ok(INode::FileLike(
                Arc::new(ProcPidStatus { pid: self.pid }) as Arc<dyn FileLike>
            )),
            "cmdline" => Ok(INode::FileLike(
                Arc::new(ProcPidCmdline { pid: self.pid }) as Arc<dyn FileLike>
            )),
            "comm" => Ok(INode::FileLike(
                Arc::new(ProcPidComm { pid: self.pid }) as Arc<dyn FileLike>
            )),
            "exe" => {
                // Symlink to executable (stub: returns /bin/unknown).
                let cmdline = Process::find_by_pid(self.pid)
                    .map(|p| p.cmdline().argv0().to_string())
                    .unwrap_or_else(|| alloc::string::String::from("/bin/unknown"));
                Ok(INode::FileLike(
                    Arc::new(ProcPidExeStub(cmdline)) as Arc<dyn FileLike>
                ))
            }
            _ => Err(Error::new(Errno::ENOENT)),
        }
    }

    fn create_file(&self, _name: &str, _mode: FileMode) -> Result<INode> {
        Err(Error::new(Errno::EPERM))
    }

    fn create_dir(&self, _name: &str, _mode: FileMode) -> Result<INode> {
        Err(Error::new(Errno::EPERM))
    }

    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFDIR | 0o555),
            ..Stat::zeroed()
        })
    }

    fn readdir(&self, index: usize) -> Result<Option<DirEntry>> {
        let entries = ["stat", "status", "cmdline", "comm", "exe"];
        if index >= entries.len() {
            return Ok(None);
        }
        Ok(Some(DirEntry {
            inode_no: INodeNo::new(0),
            file_type: FileType::Regular,
            name: alloc::string::String::from(entries[index]),
        }))
    }

    fn link(&self, _name: &str, _link_to: &INode) -> Result<()> {
        Err(Error::new(Errno::EPERM))
    }
}

// ── /proc/<pid>/stat ────────────────────────────────────────────────

struct ProcPidStat {
    pid: PId,
}

impl fmt::Debug for ProcPidStat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProcPidStat({})", self.pid.as_i32())
    }
}

impl FileLike for ProcPidStat {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFREG | 0o444),
            ..Stat::zeroed()
        })
    }

    fn read(&self, offset: usize, buf: UserBufferMut<'_>, _options: &OpenOptions) -> Result<usize> {
        if offset > 0 {
            return Ok(0);
        }

        use core::fmt::Write;
        let mut s = alloc::string::String::new();

        let pid = self.pid.as_i32();
        let comm = Process::find_by_pid(self.pid)
            .map(|p| p.cmdline().argv0().to_string())
            .unwrap_or_else(|| alloc::string::String::from("unknown"));
        let ppid = Process::find_by_pid(self.pid)
            .map(|p| p.ppid().as_i32())
            .unwrap_or(0);

        // Minimal /proc/[pid]/stat format (fields 1-52, most zeroed).
        // pid (comm) state ppid pgrp session tty_nr tpgid flags ...
        let _ = write!(
            s,
            "{pid} ({comm}) S {ppid} {pid} {pid} 0 -1 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n"
        );

        let bytes = s.as_bytes();
        let len = core::cmp::min(bytes.len(), buf.len());
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&bytes[..len])?;
        Ok(len)
    }
}

// ── /proc/<pid>/status ──────────────────────────────────────────────

struct ProcPidStatus {
    pid: PId,
}

impl fmt::Debug for ProcPidStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProcPidStatus({})", self.pid.as_i32())
    }
}

impl FileLike for ProcPidStatus {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFREG | 0o444),
            ..Stat::zeroed()
        })
    }

    fn read(&self, offset: usize, buf: UserBufferMut<'_>, _options: &OpenOptions) -> Result<usize> {
        if offset > 0 {
            return Ok(0);
        }

        use core::fmt::Write;
        let mut s = alloc::string::String::new();

        let pid = self.pid.as_i32();
        let comm = Process::find_by_pid(self.pid)
            .map(|p| p.cmdline().argv0().to_string())
            .unwrap_or_else(|| alloc::string::String::from("unknown"));
        let ppid = Process::find_by_pid(self.pid)
            .map(|p| p.ppid().as_i32())
            .unwrap_or(0);

        let _ = write!(s, "Name:\t{comm}\n");
        let _ = write!(s, "State:\tS (sleeping)\n");
        let _ = write!(s, "Pid:\t{pid}\n");
        let _ = write!(s, "PPid:\t{ppid}\n");
        let _ = write!(s, "Uid:\t0\t0\t0\t0\n");
        let _ = write!(s, "Gid:\t0\t0\t0\t0\n");

        let bytes = s.as_bytes();
        let len = core::cmp::min(bytes.len(), buf.len());
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&bytes[..len])?;
        Ok(len)
    }
}

// ── /proc/<pid>/cmdline ─────────────────────────────────────────────

struct ProcPidCmdline {
    pid: PId,
}

impl fmt::Debug for ProcPidCmdline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProcPidCmdline({})", self.pid.as_i32())
    }
}

impl FileLike for ProcPidCmdline {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFREG | 0o444),
            ..Stat::zeroed()
        })
    }

    fn read(&self, offset: usize, buf: UserBufferMut<'_>, _options: &OpenOptions) -> Result<usize> {
        if offset > 0 {
            return Ok(0);
        }

        let cmdline = Process::find_by_pid(self.pid)
            .map(|p| p.cmdline().as_str().to_string())
            .unwrap_or_default();

        // /proc/[pid]/cmdline uses NUL as separator between arguments.
        let bytes: alloc::vec::Vec<u8> = cmdline
            .as_bytes()
            .iter()
            .map(|&b| if b == b' ' { 0 } else { b })
            .chain(core::iter::once(0))
            .collect();

        let len = core::cmp::min(bytes.len(), buf.len());
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&bytes[..len])?;
        Ok(len)
    }
}

// ── /proc/<pid>/comm ────────────────────────────────────────────────

struct ProcPidComm {
    pid: PId,
}

impl fmt::Debug for ProcPidComm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProcPidComm({})", self.pid.as_i32())
    }
}

impl FileLike for ProcPidComm {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFREG | 0o444),
            ..Stat::zeroed()
        })
    }

    fn read(&self, offset: usize, buf: UserBufferMut<'_>, _options: &OpenOptions) -> Result<usize> {
        if offset > 0 {
            return Ok(0);
        }

        let comm = Process::find_by_pid(self.pid)
            .map(|p| p.cmdline().argv0().to_string())
            .unwrap_or_else(|| alloc::string::String::from("unknown"));
        let s = alloc::format!("{}\n", comm);

        let len = core::cmp::min(s.len(), buf.len());
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&s.as_bytes()[..len])?;
        Ok(len)
    }
}

// ── /proc/<pid>/exe (stub) ──────────────────────────────────────────

struct ProcPidExeStub(alloc::string::String);

impl fmt::Debug for ProcPidExeStub {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProcPidExeStub({})", self.0)
    }
}

impl FileLike for ProcPidExeStub {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFLNK | 0o777),
            ..Stat::zeroed()
        })
    }

    fn readlink(&self) -> Result<PathBuf> {
        Ok(PathBuf::from(self.0.clone()))
    }
}
