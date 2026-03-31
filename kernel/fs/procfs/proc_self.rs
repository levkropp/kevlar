// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! /proc/self symlink and /proc/[pid]/ per-process directories.
use core::fmt;

use alloc::borrow::Cow;
use alloc::string::{String, ToString};
use alloc::sync::Arc;

use kevlar_vfs::{
    inode::{DirEntry, Directory, FileLike, FileType, INode, INodeNo, OpenOptions, Symlink},
    result::{Errno, Error, Result},
    stat::{FileMode, GId, Stat, UId, S_IFLNK, S_IFDIR, S_IFREG},
    user_buffer::{UserBufWriter, UserBufferMut},
};

use crate::mm::vm::VmAreaType;
use crate::process::{current_process, Process, PId, ProcessState};

use kevlar_platform::arch::PAGE_SIZE;

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

    fn linked_to(&self) -> Result<Cow<'_, str>> {
        let pid = current_process().pid().as_i32();
        Ok(Cow::Owned(alloc::format!("/proc/{}", pid)))
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
    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            // Use a unique inode number so the VFS doesn't confuse this with
            // a mount point (mount table is keyed by inode number).
            inode_no: INodeNo::new(0x70000000 + self.pid.as_i32() as usize),
            mode: FileMode::new(S_IFDIR | 0o555),
            ..Stat::zeroed()
        })
    }

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
            "maps" => Ok(INode::FileLike(
                Arc::new(ProcPidMaps { pid: self.pid }) as Arc<dyn FileLike>
            )),
            "fd" => Ok(INode::Directory(
                Arc::new(ProcPidFdDir { pid: self.pid }) as Arc<dyn Directory>
            )),
            "cgroup" => Ok(INode::FileLike(
                Arc::new(ProcPidCgroup { pid: self.pid }) as Arc<dyn FileLike>
            )),
            "mountinfo" => Ok(INode::FileLike(
                Arc::new(ProcPidMountinfo { pid: self.pid }) as Arc<dyn FileLike>
            )),
            "environ" => Ok(INode::FileLike(
                Arc::new(ProcPidEnviron { pid: self.pid }) as Arc<dyn FileLike>
            )),
            "exe" => {
                // Symlink to the resolved executable path.
                let exe = Process::find_by_pid(self.pid)
                    .map(|p| p.exe_path_string())
                    .unwrap_or_else(|| String::from("/bin/unknown"));
                Ok(INode::Symlink(
                    Arc::new(ProcPidExeLink(exe)) as Arc<dyn Symlink>
                ))
            }
            "cwd" => {
                let cwd = Process::find_by_pid(self.pid)
                    .map(|p| {
                        let fs = p.root_fs();
                        let path = fs.lock().cwd_path().resolve_absolute_path();
                        path.as_str().to_string()
                    })
                    .unwrap_or_else(|| String::from("/"));
                Ok(INode::Symlink(
                    Arc::new(ProcPidExeLink(cwd)) as Arc<dyn Symlink>
                ))
            }
            "root" => {
                Ok(INode::Symlink(
                    Arc::new(ProcPidExeLink(String::from("/"))) as Arc<dyn Symlink>
                ))
            }
            "limits" => Ok(INode::FileLike(
                Arc::new(ProcPidLimits { pid: self.pid }) as Arc<dyn FileLike>
            )),
            _ => Err(Error::new(Errno::ENOENT)),
        }
    }

    fn create_file(&self, _name: &str, _mode: FileMode, _uid: UId, _gid: GId) -> Result<INode> {
        Err(Error::new(Errno::EPERM))
    }

    fn create_dir(&self, _name: &str, _mode: FileMode, _uid: UId, _gid: GId) -> Result<INode> {
        Err(Error::new(Errno::EPERM))
    }

    fn readdir(&self, index: usize) -> Result<Option<DirEntry>> {
        let entries: &[(&str, FileType)] = &[
            ("stat", FileType::Regular),
            ("status", FileType::Regular),
            ("cmdline", FileType::Regular),
            ("comm", FileType::Regular),
            ("exe", FileType::Link),
            ("maps", FileType::Regular),
            ("cgroup", FileType::Regular),
            ("mountinfo", FileType::Regular),
            ("environ", FileType::Regular),
            ("fd", FileType::Directory),
        ];
        if index >= entries.len() {
            return Ok(None);
        }
        Ok(Some(DirEntry {
            inode_no: INodeNo::new(0),
            file_type: entries[index].1,
            name: String::from(entries[index].0),
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
        let proc = Process::find_by_pid(self.pid);

        let comm = proc.as_ref()
            .map(|p| p.cmdline().argv0().to_string())
            .unwrap_or_else(|| alloc::string::String::from("unknown"));
        let ppid = proc.as_ref()
            .map(|p| p.ppid().as_i32())
            .unwrap_or(0);

        let state_char = proc.as_ref()
            .map(|p| process_state_char(p.state()))
            .unwrap_or('Z');
        let utime = proc.as_ref().map(|p| p.utime()).unwrap_or(0);
        let stime = proc.as_ref().map(|p| p.stime()).unwrap_or(0);
        let num_threads = proc.as_ref().map(|p| p.count_threads()).unwrap_or(0);
        let starttime = proc.as_ref().map(|p| p.start_ticks()).unwrap_or(0);
        let vsize = proc.as_ref().map(|p| p.vm_size_bytes()).unwrap_or(0);
        let rss = vsize / PAGE_SIZE;
        let pgid = proc.as_ref()
            .map(|p| p.process_group().lock().pgid().as_i32())
            .unwrap_or(pid);
        let sid = proc.as_ref()
            .map(|p| p.session_id())
            .unwrap_or(pid);
        let tty_nr = crate::fs::devfs::tty::session_ctty_rdev(sid)
            .unwrap_or(0) as i32;

        // /proc/[pid]/stat format (fields 1-52, most zeroed).
        // Fields: pid comm state ppid pgrp session tty_nr tpgid flags
        //   minflt cminflt majflt cmajflt utime stime cutime cstime
        //   priority nice num_threads itrealvalue starttime vsize rss ...
        let _ = write!(
            s,
            "{pid} ({comm}) {state_char} {ppid} {pgid} {sid} {tty_nr} -1 0 \
             0 0 0 0 {utime} {stime} 0 0 \
             20 0 {num_threads} 0 {starttime} {vsize} {rss} \
             0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n"
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
        let mut s = String::new();

        let pid = self.pid.as_i32();
        let proc = Process::find_by_pid(self.pid);

        let comm = proc.as_ref()
            .map(|p| p.cmdline().argv0().to_string())
            .unwrap_or_else(|| String::from("unknown"));
        let ppid = proc.as_ref()
            .map(|p| p.ppid().as_i32())
            .unwrap_or(0);

        let _ = write!(s, "Name:\t{comm}\n");

        let state_str = proc.as_ref()
            .map(|p| process_state_str(p.state()))
            .unwrap_or("Z (zombie)");
        let _ = write!(s, "State:\t{state_str}\n");
        let _ = write!(s, "Tgid:\t{pid}\n");
        let _ = write!(s, "Pid:\t{pid}\n");
        let _ = write!(s, "PPid:\t{ppid}\n");

        if let Some(ref p) = proc {
            let uid = p.uid();
            let euid = p.euid();
            let gid = p.gid();
            let egid = p.egid();
            let _ = write!(s, "Uid:\t{uid}\t{euid}\t{euid}\t{euid}\n");
            let _ = write!(s, "Gid:\t{gid}\t{egid}\t{egid}\t{egid}\n");

            let (fd_size, num_open) = {
                let ft = p.opened_files().lock();
                (ft.table_size(), ft.count_open())
            };
            let _ = write!(s, "FDSize:\t{}\n", fd_size);

            let vsize_bytes = p.vm_size_bytes();
            let vm_size_kb = vsize_bytes / 1024;
            let _ = write!(s, "VmSize:\t{} kB\n", vm_size_kb);
            let _ = write!(s, "VmRSS:\t{} kB\n", vm_size_kb);

            let _ = write!(s, "Threads:\t{}\n", p.count_threads());

            // Signal masks.
            let sig_pending = p.signal_pending_bits() as u64;
            let sig_blocked = p.sigset_load().bits();
            let _ = write!(s, "SigPnd:\t{:016x}\n", sig_pending);
            let _ = write!(s, "SigBlk:\t{:016x}\n", sig_blocked);

            let _ = num_open; // used above for FDSize context
        } else {
            let _ = write!(s, "Uid:\t0\t0\t0\t0\n");
            let _ = write!(s, "Gid:\t0\t0\t0\t0\n");
        }

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
            .map(|p| {
                let c = p.get_comm();
                alloc::string::String::from_utf8_lossy(&c).into_owned()
            })
            .unwrap_or_else(|| alloc::string::String::from("unknown"));
        let s = alloc::format!("{}\n", comm);

        let len = core::cmp::min(s.len(), buf.len());
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&s.as_bytes()[..len])?;
        Ok(len)
    }
}

// ── /proc/<pid>/maps ────────────────────────────────────────────────

struct ProcPidMaps {
    pid: PId,
}

impl fmt::Debug for ProcPidMaps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProcPidMaps({})", self.pid.as_i32())
    }
}

impl FileLike for ProcPidMaps {
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
        let mut s = String::new();

        if let Some(proc) = Process::find_by_pid(self.pid) {
            if let Some(ref vm_arc) = *proc.vm() {
                let vm = vm_arc.lock();
                for (i, vma) in vm.vm_areas().iter().enumerate() {
                    let start = vma.start().value();
                    let end = vma.end().value();
                    let prot = vma.prot();

                    let r = if prot.contains(crate::ctypes::MMapProt::PROT_READ) { 'r' } else { '-' };
                    let w = if prot.contains(crate::ctypes::MMapProt::PROT_WRITE) { 'w' } else { '-' };
                    let x = if prot.contains(crate::ctypes::MMapProt::PROT_EXEC) { 'x' } else { '-' };

                    let (offset_val, name) = match vma.area_type() {
                        VmAreaType::File { offset, .. } => {
                            (*offset, String::new())
                        }
                        VmAreaType::Anonymous => {
                            // vm_areas[0] = stack, vm_areas[1] = heap (see Vm::new).
                            let label = if i == 0 {
                                "[stack]"
                            } else if i == 1 {
                                "[heap]"
                            } else {
                                ""
                            };
                            (0, String::from(label))
                        }
                    };

                    let _ = write!(
                        s,
                        "{:08x}-{:08x} {}{}{}p {:08x} 00:00 0",
                        start, end, r, w, x, offset_val,
                    );
                    if !name.is_empty() {
                        let _ = write!(s, "          {}", name);
                    }
                    let _ = write!(s, "\n");
                }
            }

            // Synthetic vDSO entry (mapped directly into page table, not a VMA).
            #[cfg(target_arch = "x86_64")]
            {
                let vdso_addr = kevlar_platform::arch::vdso::VDSO_VADDR;
                let _ = write!(
                    s,
                    "{:08x}-{:08x} r-xp 00000000 00:00 0          [vdso]\n",
                    vdso_addr, vdso_addr + PAGE_SIZE,
                );
            }
        }

        let bytes = s.as_bytes();
        let len = core::cmp::min(bytes.len(), buf.len());
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&bytes[..len])?;
        Ok(len)
    }
}

// ── /proc/<pid>/fd/ directory ───────────────────────────────────────

struct ProcPidFdDir {
    pid: PId,
}

impl fmt::Debug for ProcPidFdDir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProcPidFdDir({})", self.pid.as_i32())
    }
}

impl Directory for ProcPidFdDir {
    fn lookup(&self, name: &str) -> Result<INode> {
        let fd_num: usize = name.parse().map_err(|_| Error::new(Errno::ENOENT))?;
        let proc = Process::find_by_pid(self.pid).ok_or_else(|| Error::new(Errno::ENOENT))?;
        // Return a symlink so the VFS follows it during path resolution.
        // We resolve the target path here (holding the fd table lock briefly)
        // and return a Symlink INode that the VFS follows without re-locking.
        let target = {
            let opened_files = proc.opened_files().lock();
            let file = opened_files.get(crate::fs::opened_file::Fd::new(fd_num as i32))?;
            file.path().resolve_absolute_path().as_str().to_string()
        };
        Ok(INode::Symlink(
            Arc::new(ProcFdLink(target)) as Arc<dyn Symlink>
        ))
    }

    fn create_file(&self, _name: &str, _mode: FileMode, _uid: UId, _gid: GId) -> Result<INode> {
        Err(Error::new(Errno::EPERM))
    }

    fn create_dir(&self, _name: &str, _mode: FileMode, _uid: UId, _gid: GId) -> Result<INode> {
        Err(Error::new(Errno::EPERM))
    }

    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            inode_no: INodeNo::new(0x71000000 + self.pid.as_i32() as usize),
            mode: FileMode::new(S_IFDIR | 0o555),
            ..Stat::zeroed()
        })
    }

    fn readdir(&self, index: usize) -> Result<Option<DirEntry>> {
        let proc = match Process::find_by_pid(self.pid) {
            Some(p) => p,
            None => return Ok(None),
        };
        let opened_files = proc.opened_files().lock();
        let mut count = 0;
        for (fd_num, _file) in opened_files.iter_open() {
            if count == index {
                return Ok(Some(DirEntry {
                    inode_no: INodeNo::new(0),
                    file_type: FileType::Link,
                    name: alloc::format!("{}", fd_num),
                }));
            }
            count += 1;
        }
        Ok(None)
    }

    fn link(&self, _name: &str, _link_to: &INode) -> Result<()> {
        Err(Error::new(Errno::EPERM))
    }
}

/// Symlink for /proc/[pid]/fd/N → target path (VFS-resolvable).
struct ProcFdLink(String);

impl fmt::Debug for ProcFdLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProcFdLink({})", self.0)
    }
}

impl Symlink for ProcFdLink {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFLNK | 0o777),
            ..Stat::zeroed()
        })
    }

    fn linked_to(&self) -> Result<Cow<'_, str>> {
        Ok(Cow::Borrowed(&self.0))
    }
}

/// Legacy symlink for /proc/[pid]/fd/N → target path (FileLike for readlink).
#[allow(dead_code)]
struct ProcFdSymlink(String);

impl fmt::Debug for ProcFdSymlink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProcFdSymlink({})", self.0)
    }
}

impl FileLike for ProcFdSymlink {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFLNK | 0o777),
            ..Stat::zeroed()
        })
    }

    fn readlink(&self) -> Result<Cow<'_, str>> {
        Ok(Cow::Borrowed(&self.0))
    }
}

// ── /proc/<pid>/exe (symlink to resolved executable) ────────────────

struct ProcPidExeLink(String);

impl fmt::Debug for ProcPidExeLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProcPidExeLink({})", self.0)
    }
}

impl Symlink for ProcPidExeLink {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFLNK | 0o777),
            ..Stat::zeroed()
        })
    }

    fn linked_to(&self) -> Result<Cow<'_, str>> {
        Ok(Cow::Borrowed(&self.0))
    }
}

// ── /proc/<pid>/limits ──────────────────────────────────────────────

struct ProcPidLimits {
    pid: PId,
}

impl fmt::Debug for ProcPidLimits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProcPidLimits({})", self.pid.as_i32())
    }
}

impl FileLike for ProcPidLimits {
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
        let mut s = String::new();
        let _ = writeln!(s, "{:<26}{:<21}{:<21}{}", "Limit", "Soft Limit", "Hard Limit", "Units");

        let rl = Process::find_by_pid(self.pid)
            .map(|p| p.rlimits())
            .unwrap_or_else(|| {
                const INF: u64 = !0u64;
                [[INF; 2]; 16]
            });

        const NAMES: [(&str, &str); 16] = [
            ("Max cpu time", "seconds"),
            ("Max file size", "bytes"),
            ("Max data size", "bytes"),
            ("Max stack size", "bytes"),
            ("Max core file size", "bytes"),
            ("Max resident set", "bytes"),
            ("Max processes", "processes"),
            ("Max open files", "files"),
            ("Max locked memory", "bytes"),
            ("Max address space", "bytes"),
            ("Max file locks", "locks"),
            ("Max pending signals", "signals"),
            ("Max msgqueue size", "bytes"),
            ("Max nice priority", ""),
            ("Max realtime priority", ""),
            ("Max realtime timeout", "us"),
        ];

        for (i, (name, units)) in NAMES.iter().enumerate() {
            let fmt_val = |v: u64| -> String {
                if v == !0u64 { String::from("unlimited") } else { alloc::format!("{}", v) }
            };
            let _ = writeln!(s, "{:<26}{:<21}{:<21}{}", name, fmt_val(rl[i][0]), fmt_val(rl[i][1]), units);
        }

        let bytes = s.as_bytes();
        let len = core::cmp::min(bytes.len(), buf.len());
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&bytes[..len])?;
        Ok(len)
    }
}

// ── /proc/<pid>/environ ─────────────────────────────────────────────

struct ProcPidEnviron {
    pid: PId,
}

impl fmt::Debug for ProcPidEnviron {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProcPidEnviron({})", self.pid.as_i32())
    }
}

impl FileLike for ProcPidEnviron {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFREG | 0o444),
            ..Stat::zeroed()
        })
    }

    fn read(&self, offset: usize, buf: UserBufferMut<'_>, _options: &OpenOptions) -> Result<usize> {
        if offset > 0 { return Ok(0); }
        // Return a synthetic per-process environment string so that OpenRC's
        // md5sum-based /proc liveness check (init.sh) sees unique content per
        // process and correctly detects that /proc is real.
        let pid = current_process().pid().as_i32();
        let s = alloc::format!("KEVLAR_PID={}\0", pid);
        let bytes = s.as_bytes();
        let len = core::cmp::min(bytes.len(), buf.len());
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&bytes[..len])?;
        Ok(len)
    }
}

// ── /proc/<pid>/mountinfo ───────────────────────────────────────────

struct ProcPidMountinfo {
    pid: PId,
}

impl fmt::Debug for ProcPidMountinfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProcPidMountinfo({})", self.pid.as_i32())
    }
}

impl FileLike for ProcPidMountinfo {
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

        let mut s = alloc::string::String::new();
        crate::fs::mount::MountTable::format_mountinfo(&mut s);

        let bytes = s.as_bytes();
        let len = core::cmp::min(bytes.len(), buf.len());
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&bytes[..len])?;
        Ok(len)
    }
}

// ── /proc/<pid>/cgroup ──────────────────────────────────────────────

struct ProcPidCgroup {
    pid: PId,
}

impl fmt::Debug for ProcPidCgroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProcPidCgroup({})", self.pid.as_i32())
    }
}

impl FileLike for ProcPidCgroup {
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

        let path = Process::find_by_pid(self.pid)
            .map(|p| p.cgroup().path())
            .unwrap_or_else(|| alloc::string::String::from("/"));

        let s = alloc::format!("0::{}\n", path);
        // Trace disabled — uncomment for cgroup debugging:
        // warn!("/proc/{}/cgroup: {:?}", self.pid.as_i32(), s);
        let bytes = s.as_bytes();
        let len = core::cmp::min(bytes.len(), buf.len());
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&bytes[..len])?;
        Ok(len)
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Map ProcessState to single-character code for /proc/[pid]/stat field 3.
fn process_state_char(state: ProcessState) -> char {
    match state {
        ProcessState::Runnable => 'R',
        ProcessState::BlockedSignalable => 'S',
        ProcessState::Stopped(_) => 'T',
        ProcessState::ExitedWith(_) | ProcessState::ExitedBySignal(_) => 'Z',
    }
}

/// Map ProcessState to human-readable string for /proc/[pid]/status.
fn process_state_str(state: ProcessState) -> &'static str {
    match state {
        ProcessState::Runnable => "R (running)",
        ProcessState::BlockedSignalable => "S (sleeping)",
        ProcessState::Stopped(_) => "T (stopped)",
        ProcessState::ExitedWith(_) | ProcessState::ExitedBySignal(_) => "Z (zombie)",
    }
}
