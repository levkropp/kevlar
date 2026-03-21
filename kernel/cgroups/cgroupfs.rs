// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! cgroups v2 filesystem implementation.
use super::{CgroupNode, CGROUP_ROOT, CTRL_CPU, CTRL_MEMORY, CTRL_PIDS};
use crate::process::{PId, Process};
use alloc::string::String;
use alloc::sync::Arc;
use core::fmt;
use core::sync::atomic::Ordering;
use kevlar_vfs::{
    file_system::FileSystem,
    inode::{DirEntry, Directory, FileLike, FileType, INode, INodeNo, OpenOptions},
    result::{Errno, Error, Result},
    stat::{FileMode, GId, Stat, UId, S_IFDIR, S_IFREG},
    user_buffer::{UserBufReader, UserBufWriter, UserBuffer, UserBufferMut},
};

/// Singleton cgroup2 filesystem.
pub struct CgroupFs {
    root: Arc<CgroupNode>,
}

impl CgroupFs {
    pub fn new_or_get() -> Arc<CgroupFs> {
        Arc::new(CgroupFs {
            root: CGROUP_ROOT.clone(),
        })
    }
}

impl FileSystem for CgroupFs {
    fn root_dir(&self) -> Result<Arc<dyn Directory>> {
        Ok(Arc::new(CgroupDir {
            node: self.root.clone(),
        }) as Arc<dyn Directory>)
    }
}

// ── CgroupDir: directory for a cgroup node ──────────────────────────

struct CgroupDir {
    node: Arc<CgroupNode>,
}

impl fmt::Debug for CgroupDir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CgroupDir({})", self.node.path())
    }
}

/// Control file entries always present in every cgroup directory.
const BASE_ENTRIES: &[(&str, FileType)] = &[
    ("cgroup.procs", FileType::Regular),
    ("cgroup.controllers", FileType::Regular),
    ("cgroup.subtree_control", FileType::Regular),
    ("cgroup.type", FileType::Regular),
    ("cgroup.stat", FileType::Regular),
];

/// Controller-specific file entries.
const PIDS_ENTRIES: &[(&str, FileType)] = &[
    ("pids.max", FileType::Regular),
    ("pids.current", FileType::Regular),
];

const MEMORY_ENTRIES: &[(&str, FileType)] = &[
    ("memory.max", FileType::Regular),
    ("memory.current", FileType::Regular),
];

const CPU_ENTRIES: &[(&str, FileType)] = &[
    ("cpu.max", FileType::Regular),
    ("cpu.stat", FileType::Regular),
];

impl Directory for CgroupDir {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            inode_no: INodeNo::new(0x80000000 + (&*self.node as *const CgroupNode as usize & 0x0FFFFFFF)),
            mode: FileMode::new(S_IFDIR | 0o755),
            ..Stat::zeroed()
        })
    }

    fn lookup(&self, name: &str) -> Result<INode> {
        // Check for child cgroup directories.
        if let Some(child) = self.node.children.lock().get(name) {
            return Ok(INode::Directory(
                Arc::new(CgroupDir { node: child.clone() }) as Arc<dyn Directory>,
            ));
        }

        // Check for control files.
        if let Some(kind) = file_kind_from_name(name, self.node.available_controllers()) {
            return Ok(INode::FileLike(
                Arc::new(CgroupControlFile {
                    node: self.node.clone(),
                    kind,
                }) as Arc<dyn FileLike>,
            ));
        }

        Err(Error::new(Errno::ENOENT))
    }

    fn create_file(&self, _name: &str, _mode: FileMode, _uid: UId, _gid: GId) -> Result<INode> {
        Err(Error::new(Errno::EPERM))
    }

    fn create_dir(&self, name: &str, _mode: FileMode, _uid: UId, _gid: GId) -> Result<INode> {
        let mut children = self.node.children.lock();
        if children.contains_key(name) {
            return Err(Error::new(Errno::EEXIST));
        }
        let child = CgroupNode::new_child(name, &self.node);
        children.insert(String::from(name), child.clone());
        Ok(INode::Directory(
            Arc::new(CgroupDir { node: child }) as Arc<dyn Directory>,
        ))
    }

    fn rmdir(&self, name: &str) -> Result<()> {
        let mut children = self.node.children.lock();
        if let Some(child) = children.get(name) {
            if !child.member_pids.lock().is_empty() {
                return Err(Error::new(Errno::EBUSY));
            }
            if !child.children.lock().is_empty() {
                return Err(Error::new(Errno::EBUSY));
            }
            children.remove(name);
            Ok(())
        } else {
            Err(Error::new(Errno::ENOENT))
        }
    }

    fn readdir(&self, index: usize) -> Result<Option<DirEntry>> {
        let avail = self.node.available_controllers();
        let mut entries: alloc::vec::Vec<(&str, FileType)> = alloc::vec::Vec::new();

        // Child cgroup directories.
        let children = self.node.children.lock();
        for _name in children.keys() {
            entries.push(("", FileType::Directory)); // placeholder
            // We'll handle this specially below.
        }
        let child_names: alloc::vec::Vec<String> = children.keys().cloned().collect();
        drop(children);

        // Rebuild with actual entries.
        entries.clear();

        // First: child directories.
        for name in &child_names {
            entries.push((name.as_str(), FileType::Directory));
        }

        // Then: base control files.
        entries.extend_from_slice(BASE_ENTRIES);

        // Controller-specific files.
        if avail & CTRL_PIDS != 0 {
            entries.extend_from_slice(PIDS_ENTRIES);
        }
        if avail & CTRL_MEMORY != 0 {
            entries.extend_from_slice(MEMORY_ENTRIES);
        }
        if avail & CTRL_CPU != 0 {
            entries.extend_from_slice(CPU_ENTRIES);
        }

        if index >= entries.len() {
            return Ok(None);
        }

        // For child dirs, use the child name; for files, use the static name.
        let name = if index < child_names.len() {
            child_names[index].clone()
        } else {
            String::from(entries[index].0)
        };

        Ok(Some(DirEntry {
            inode_no: INodeNo::new(0),
            file_type: entries[index].1,
            name,
        }))
    }

    fn link(&self, _name: &str, _link_to: &INode) -> Result<()> {
        Err(Error::new(Errno::EPERM))
    }
}

// ── CgroupControlFile: read/write for control files ─────────────────

#[derive(Debug, Clone, Copy)]
enum CgroupFileKind {
    CgroupProcs,
    CgroupControllers,
    SubtreeControl,
    CgroupType,
    CgroupStat,
    PidsMax,
    PidsCurrent,
    MemoryMax,
    MemoryCurrent,
    CpuMax,
    CpuStat,
}

fn file_kind_from_name(name: &str, avail: u32) -> Option<CgroupFileKind> {
    match name {
        "cgroup.procs" => Some(CgroupFileKind::CgroupProcs),
        "cgroup.controllers" => Some(CgroupFileKind::CgroupControllers),
        "cgroup.subtree_control" => Some(CgroupFileKind::SubtreeControl),
        "cgroup.type" => Some(CgroupFileKind::CgroupType),
        "cgroup.stat" => Some(CgroupFileKind::CgroupStat),
        "pids.max" if avail & CTRL_PIDS != 0 => Some(CgroupFileKind::PidsMax),
        "pids.current" if avail & CTRL_PIDS != 0 => Some(CgroupFileKind::PidsCurrent),
        "memory.max" if avail & CTRL_MEMORY != 0 => Some(CgroupFileKind::MemoryMax),
        "memory.current" if avail & CTRL_MEMORY != 0 => Some(CgroupFileKind::MemoryCurrent),
        "cpu.max" if avail & CTRL_CPU != 0 => Some(CgroupFileKind::CpuMax),
        "cpu.stat" if avail & CTRL_CPU != 0 => Some(CgroupFileKind::CpuStat),
        _ => None,
    }
}

struct CgroupControlFile {
    node: Arc<CgroupNode>,
    kind: CgroupFileKind,
}

impl fmt::Debug for CgroupControlFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CgroupControlFile({:?})", self.kind)
    }
}

impl FileLike for CgroupControlFile {
    fn stat(&self) -> Result<Stat> {
        let mode = match self.kind {
            CgroupFileKind::CgroupProcs
            | CgroupFileKind::SubtreeControl
            | CgroupFileKind::PidsMax
            | CgroupFileKind::MemoryMax
            | CgroupFileKind::CpuMax => S_IFREG | 0o644, // writable
            _ => S_IFREG | 0o444, // read-only
        };
        Ok(Stat {
            mode: FileMode::new(mode),
            ..Stat::zeroed()
        })
    }

    fn read(&self, offset: usize, buf: UserBufferMut<'_>, _options: &OpenOptions) -> Result<usize> {
        if offset > 0 {
            return Ok(0);
        }

        use core::fmt::Write;
        let mut s = String::new();

        match self.kind {
            CgroupFileKind::CgroupProcs => {
                for pid in self.node.member_pids.lock().iter() {
                    let _ = write!(s, "{}\n", pid.as_i32());
                }
            }
            CgroupFileKind::CgroupControllers => {
                let avail = self.node.available_controllers();
                let mut first = true;
                if avail & CTRL_CPU != 0 {
                    s.push_str("cpu");
                    first = false;
                }
                if avail & CTRL_MEMORY != 0 {
                    if !first { s.push(' '); }
                    s.push_str("memory");
                    first = false;
                }
                if avail & CTRL_PIDS != 0 {
                    if !first { s.push(' '); }
                    s.push_str("pids");
                }
                s.push('\n');
            }
            CgroupFileKind::SubtreeControl => {
                let ctrl = self.node.subtree_control.load(Ordering::Relaxed);
                let mut first = true;
                if ctrl & CTRL_CPU != 0 {
                    s.push_str("cpu");
                    first = false;
                }
                if ctrl & CTRL_MEMORY != 0 {
                    if !first { s.push(' '); }
                    s.push_str("memory");
                    first = false;
                }
                if ctrl & CTRL_PIDS != 0 {
                    if !first { s.push(' '); }
                    s.push_str("pids");
                }
                s.push('\n');
            }
            CgroupFileKind::CgroupType => {
                s.push_str("domain\n");
            }
            CgroupFileKind::CgroupStat => {
                let nr_desc = self.node.children.lock().len();
                let _ = write!(s, "nr_descendants {}\nnr_dying_descendants 0\n", nr_desc);
            }
            CgroupFileKind::PidsMax => {
                let max = self.node.pids_max.load(Ordering::Relaxed);
                if max < 0 {
                    s.push_str("max\n");
                } else {
                    let _ = write!(s, "{}\n", max);
                }
            }
            CgroupFileKind::PidsCurrent => {
                let count = self.node.count_pids_recursive();
                let _ = write!(s, "{}\n", count);
            }
            CgroupFileKind::MemoryMax => {
                let max = self.node.memory_max.load(Ordering::Relaxed);
                if max < 0 {
                    s.push_str("max\n");
                } else {
                    let _ = write!(s, "{}\n", max);
                }
            }
            CgroupFileKind::MemoryCurrent => {
                s.push_str("0\n");
            }
            CgroupFileKind::CpuMax => {
                let quota = self.node.cpu_max_quota.load(Ordering::Relaxed);
                let period = self.node.cpu_max_period.load(Ordering::Relaxed);
                if quota < 0 {
                    let _ = write!(s, "max {}\n", period);
                } else {
                    let _ = write!(s, "{} {}\n", quota, period);
                }
            }
            CgroupFileKind::CpuStat => {
                s.push_str("usage_usec 0\nuser_usec 0\nsystem_usec 0\n");
            }
        }

        let bytes = s.as_bytes();
        let len = core::cmp::min(bytes.len(), buf.len());
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&bytes[..len])?;
        Ok(len)
    }

    fn write(&self, _offset: usize, buf: UserBuffer<'_>, _options: &OpenOptions) -> Result<usize> {
        // Read user input.
        let mut data = [0u8; 128];
        let mut reader = UserBufReader::from(buf);
        let n = reader.read_bytes(&mut data)?;
        let input = core::str::from_utf8(&data[..n]).map_err(|_| Error::new(Errno::EINVAL))?;
        let input = input.trim();

        match self.kind {
            CgroupFileKind::CgroupProcs => {
                let pid: i32 = input.parse().map_err(|_| Error::new(Errno::EINVAL))?;
                let pid = PId::new(pid);
                let proc = Process::find_by_pid(pid).ok_or_else(|| Error::new(Errno::ESRCH))?;

                // Remove from old cgroup.
                {
                    let old_cgroup = proc.cgroup();
                    old_cgroup.member_pids.lock().retain(|p| *p != pid);
                }

                // Add to new cgroup.
                self.node.member_pids.lock().push(pid);

                // Update process's cgroup reference.
                proc.set_cgroup(self.node.clone());
            }
            CgroupFileKind::SubtreeControl => {
                // Parse "+cpu -memory +pids" format.
                let avail = self.node.available_controllers();
                let mut ctrl = self.node.subtree_control.load(Ordering::Relaxed);
                for token in input.split_whitespace() {
                    let (add, name) = if let Some(name) = token.strip_prefix('+') {
                        (true, name)
                    } else if let Some(name) = token.strip_prefix('-') {
                        (false, name)
                    } else {
                        continue;
                    };
                    let bit = match name {
                        "cpu" => CTRL_CPU,
                        "memory" => CTRL_MEMORY,
                        "pids" => CTRL_PIDS,
                        _ => return Err(Error::new(Errno::EINVAL)),
                    };
                    if add && (avail & bit == 0) {
                        return Err(Error::new(Errno::EINVAL)); // controller not available
                    }
                    if add {
                        ctrl |= bit;
                    } else {
                        ctrl &= !bit;
                    }
                }
                self.node.subtree_control.store(ctrl, Ordering::Relaxed);
            }
            CgroupFileKind::PidsMax => {
                if input == "max" {
                    self.node.pids_max.store(-1, Ordering::Relaxed);
                } else {
                    let max: i64 = input.parse().map_err(|_| Error::new(Errno::EINVAL))?;
                    self.node.pids_max.store(max, Ordering::Relaxed);
                }
            }
            CgroupFileKind::MemoryMax => {
                if input == "max" {
                    self.node.memory_max.store(-1, Ordering::Relaxed);
                } else {
                    let max: i64 = input.parse().map_err(|_| Error::new(Errno::EINVAL))?;
                    self.node.memory_max.store(max, Ordering::Relaxed);
                }
            }
            CgroupFileKind::CpuMax => {
                // Format: "quota period" or "max period"
                let mut parts = input.split_whitespace();
                if let Some(quota_str) = parts.next() {
                    if quota_str == "max" {
                        self.node.cpu_max_quota.store(-1, Ordering::Relaxed);
                    } else {
                        let q: i64 = quota_str.parse().map_err(|_| Error::new(Errno::EINVAL))?;
                        self.node.cpu_max_quota.store(q, Ordering::Relaxed);
                    }
                }
                if let Some(period_str) = parts.next() {
                    let p: i64 = period_str.parse().map_err(|_| Error::new(Errno::EINVAL))?;
                    self.node.cpu_max_period.store(p, Ordering::Relaxed);
                }
            }
            _ => return Err(Error::new(Errno::EACCES)),
        }

        Ok(n)
    }
}
