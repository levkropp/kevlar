// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! /proc filesystem.
//!
//! Provides per-process directories (/proc/[pid]/, /proc/self) and
//! system-wide files (/proc/mounts, /proc/meminfo, /proc/stat, etc.).
use core::fmt;

use crate::{
    fs::{
        file_system::FileSystem,
        inode::{Directory, FileLike},
    },
    process::PId,
    result::Result,
};
use alloc::sync::Arc;
use kevlar_vfs::{
    inode::{DirEntry, INode, INodeNo},
    stat::{FileMode, Stat, S_IFDIR},
};
use kevlar_utils::once::Once;

use self::metrics::MetricsFile;
use self::proc_self::{ProcPidDir, ProcSelfSymlink};
use self::system::*;

use super::tmpfs::TmpFs;

mod metrics;
pub mod proc_self;
mod system;

pub static PROC_FS: Once<Arc<ProcFs>> = Once::new();

pub struct ProcFs {
    /// Underlying tmpfs for static entries.
    tmpfs: TmpFs,
    /// Dynamic root directory that intercepts PID lookups.
    root: Arc<ProcRootDir>,
}

impl ProcFs {
    pub fn new() -> ProcFs {
        let tmpfs = TmpFs::new();
        let static_root = tmpfs.root_tmpfs_dir();

        // Add static system-wide files.
        static_root.add_file("metrics", Arc::new(MetricsFile::new()) as Arc<dyn FileLike>);
        static_root.add_file("mounts", Arc::new(ProcMountsFile) as Arc<dyn FileLike>);
        static_root.add_file("filesystems", Arc::new(ProcFilesystemsFile) as Arc<dyn FileLike>);
        static_root.add_file("cmdline", Arc::new(ProcCmdlineFile) as Arc<dyn FileLike>);
        static_root.add_file("stat", Arc::new(ProcStatFile) as Arc<dyn FileLike>);
        static_root.add_file("meminfo", Arc::new(ProcMeminfoFile) as Arc<dyn FileLike>);
        static_root.add_file("version", Arc::new(ProcVersionFile) as Arc<dyn FileLike>);

        let static_dir: Arc<dyn Directory> = tmpfs.root_dir().unwrap();

        let root = Arc::new(ProcRootDir {
            static_dir,
        });

        ProcFs { tmpfs, root }
    }
}

impl FileSystem for ProcFs {
    fn root_dir(&self) -> Result<Arc<dyn Directory>> {
        Ok(self.root.clone())
    }
}

pub fn init() {
    PROC_FS.init(|| Arc::new(ProcFs::new()));
}

// ── Dynamic /proc root directory ────────────────────────────────────

/// Root directory of /proc. Intercepts lookups for "self" and numeric
/// PID directories, delegates everything else to the static tmpfs.
struct ProcRootDir {
    static_dir: Arc<dyn Directory>,
}

impl fmt::Debug for ProcRootDir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcRootDir").finish()
    }
}

impl Directory for ProcRootDir {
    fn lookup(&self, name: &str) -> Result<INode> {
        // "self" → symlink to /proc/<current_pid>
        if name == "self" {
            return Ok(INode::Symlink(
                Arc::new(ProcSelfSymlink) as Arc<dyn kevlar_vfs::inode::Symlink>
            ));
        }

        // Numeric name → per-PID directory.
        if let Ok(pid_val) = name.parse::<i32>() {
            if pid_val > 0 {
                return Ok(INode::Directory(
                    ProcPidDir::new(PId::new(pid_val)) as Arc<dyn Directory>
                ));
            }
        }

        // Fall through to static entries (metrics, mounts, etc.).
        self.static_dir.lookup(name)
    }

    fn create_file(&self, name: &str, mode: FileMode) -> Result<INode> {
        self.static_dir.create_file(name, mode)
    }

    fn create_dir(&self, name: &str, mode: FileMode) -> Result<INode> {
        self.static_dir.create_dir(name, mode)
    }

    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFDIR | 0o555),
            ..Stat::zeroed()
        })
    }

    fn inode_no(&self) -> Result<INodeNo> {
        self.static_dir.inode_no()
    }

    fn readdir(&self, index: usize) -> Result<Option<DirEntry>> {
        // Show static entries first, then "self".
        if let Some(entry) = self.static_dir.readdir(index)? {
            return Ok(Some(entry));
        }

        // After static entries, show "self" pseudo-entry.
        // We can't easily enumerate all PIDs, but showing "self" is sufficient.
        Ok(None)
    }

    fn link(&self, name: &str, link_to: &INode) -> Result<()> {
        self.static_dir.link(name, link_to)
    }

    fn unlink(&self, name: &str) -> Result<()> {
        self.static_dir.unlink(name)
    }
}
