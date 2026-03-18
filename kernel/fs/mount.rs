// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use super::{
    file_system::FileSystem,
    inode::{Directory, FileLike, INode, MountKey},
    opened_file::OpenedFileTable,
    opened_file::PathComponent,
    path::Path,
};
use crate::prelude::*;
use crate::syscalls::CwdOrFd;

use alloc::collections::VecDeque;
use hashbrown::HashMap;
use kevlar_platform::spinlock::SpinLock;

const DEFAULT_SYMLINK_FOLLOW_MAX: usize = 8;

// ── Mount table (for /proc/mounts) ──────────────────────────────────

struct MountEntry {
    mount_id: u32,
    parent_id: u32,
    fstype: String,
    mountpoint: String,
}

static MOUNT_ENTRIES: SpinLock<VecDeque<MountEntry>> = SpinLock::new(VecDeque::new());
static NEXT_MOUNT_ID: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1);

/// Global mount table for /proc/mounts reporting.
pub struct MountTable;

impl MountTable {
    /// Register initial boot-time mounts.
    pub fn init() {
        use core::sync::atomic::Ordering;
        let mut entries = MOUNT_ENTRIES.lock();
        let root_id = NEXT_MOUNT_ID.fetch_add(1, Ordering::Relaxed);
        entries.push_back(MountEntry {
            mount_id: root_id, parent_id: 0,
            fstype: String::from("initramfs"),
            mountpoint: String::from("/"),
        });
        for (fstype, mp) in &[("proc", "/proc"), ("devtmpfs", "/dev"), ("tmpfs", "/tmp"), ("sysfs", "/sys"), ("cgroup2", "/sys/fs/cgroup")] {
            let id = NEXT_MOUNT_ID.fetch_add(1, Ordering::Relaxed);
            entries.push_back(MountEntry {
                mount_id: id, parent_id: root_id,
                fstype: String::from(*fstype),
                mountpoint: String::from(*mp),
            });
        }
    }

    pub fn add(fstype: &str, mountpoint: &str) {
        use core::sync::atomic::Ordering;
        // Find parent mount ID via longest-prefix match.
        let entries = MOUNT_ENTRIES.lock();
        let parent_id = entries.iter()
            .filter(|e| mountpoint.starts_with(e.mountpoint.as_str()))
            .max_by_key(|e| e.mountpoint.len())
            .map(|e| e.mount_id)
            .unwrap_or(0);
        drop(entries);
        let id = NEXT_MOUNT_ID.fetch_add(1, Ordering::Relaxed);
        MOUNT_ENTRIES.lock().push_back(MountEntry {
            mount_id: id, parent_id,
            fstype: String::from(fstype),
            mountpoint: String::from(mountpoint),
        });
    }

    pub fn remove(mountpoint: &str) {
        MOUNT_ENTRIES.lock().retain(|e| e.mountpoint != mountpoint);
    }

    /// Return the mount ID for the mount that owns `path`.
    /// Uses longest-prefix matching to resolve nested mounts.
    pub fn mount_id_for_path(path: &str) -> i32 {
        let entries = MOUNT_ENTRIES.lock();
        let mut best_len = 0usize;
        let mut best_id = 0i32;
        for entry in entries.iter() {
            let mp = entry.mountpoint.as_str();
            let matches = if mp == "/" {
                true
            } else {
                path.starts_with(mp)
                    && (path.len() == mp.len()
                        || path.as_bytes().get(mp.len()) == Some(&b'/'))
            };
            if matches && mp.len() >= best_len {
                best_len = mp.len();
                best_id = entry.mount_id as i32;
            }
        }
        best_id
    }

    /// Return the filesystem type for the mount that owns `path`.
    /// Performs longest-prefix matching so nested mounts resolve correctly.
    pub fn fstype_for_path(path: &str) -> Option<String> {
        let entries = MOUNT_ENTRIES.lock();
        let mut best_len = 0usize;
        let mut best_fstype: Option<String> = None;
        for entry in entries.iter() {
            let mp = entry.mountpoint.as_str();
            let matches = if mp == "/" {
                true
            } else {
                path.starts_with(mp)
                    && (path.len() == mp.len()
                        || path.as_bytes().get(mp.len()) == Some(&b'/'))
            };
            if matches && mp.len() >= best_len {
                best_len = mp.len();
                best_fstype = Some(entry.fstype.clone());
            }
        }
        best_fstype
    }

    /// Format /proc/mounts content.
    pub fn format_mounts(buf: &mut String) {
        use core::fmt::Write;
        let entries = MOUNT_ENTRIES.lock();
        for entry in entries.iter() {
            let _ = writeln!(buf, "none {} {} rw 0 0", entry.mountpoint, entry.fstype);
        }
    }

    /// Format /proc/[pid]/mountinfo content.
    pub fn format_mountinfo(buf: &mut String) {
        use core::fmt::Write;
        let entries = MOUNT_ENTRIES.lock();
        for (i, entry) in entries.iter().enumerate() {
            // Format: mount_id parent_id major:minor root mount_point options optional - fstype source super_options
            let _ = writeln!(
                buf,
                "{} {} 0:{} / {} rw - {} none rw",
                entry.mount_id, entry.parent_id, i + 1, entry.mountpoint, entry.fstype,
            );
        }
    }
}

#[derive(Clone)]
pub struct MountPoint {
    fs: Arc<dyn FileSystem>,
}

#[derive(Clone)]
pub struct RootFs {
    root_path: Arc<PathComponent>,
    cwd_path: Arc<PathComponent>,
    mount_points: HashMap<MountKey, MountPoint>,
    symlink_follow_limit: usize,
}

impl RootFs {
    pub fn new(root: Arc<dyn FileSystem>) -> Result<RootFs> {
        let root_path = Arc::new(PathComponent {
            parent_dir: None,
            name: String::new(),
            inode: root.root_dir()?.into(),
        });

        Ok(RootFs {
            mount_points: HashMap::new(),
            root_path: root_path.clone(),
            cwd_path: root_path,
            symlink_follow_limit: DEFAULT_SYMLINK_FOLLOW_MAX,
        })
    }

    pub fn mount(&mut self, dir: Arc<dyn Directory>, fs: Arc<dyn FileSystem>) -> Result<()> {
        self.mount_points
            .insert(dir.mount_key()?, MountPoint { fs });
        Ok(())
    }

    /// Resolves a path (from the current working directory) into an inode.
    /// This method resolves symbolic links: it will never return `INode::Symlink`.
    pub fn lookup(&self, path: &Path) -> Result<INode> {
        self.lookup_inode(path, true)
    }

    /// Resolves a path (from the current working directory) into an inode without
    /// following symlinks.
    pub fn lookup_no_symlink_follow(&self, path: &Path) -> Result<INode> {
        self.lookup_inode(path, false)
    }

    /// Resolves a path (from the current working directory) into an file.
    pub fn lookup_file(&self, path: &Path) -> Result<Arc<dyn FileLike>> {
        match self.lookup(path)? {
            INode::Directory(_) => Err(Error::new(Errno::EISDIR)),
            INode::FileLike(file) => Ok(file),
            // Symbolic links should be already resolved.
            INode::Symlink(_) => unreachable!(),
        }
    }

    /// Resolves a path (from the current working directory) into an directory.
    pub fn lookup_dir(&self, path: &Path) -> Result<Arc<dyn Directory>> {
        match self.lookup(path)? {
            INode::Directory(dir) => Ok(dir),
            INode::FileLike(_) => Err(Error::new(Errno::EISDIR)),
            // Symbolic links should be already resolved.
            INode::Symlink(_) => unreachable!(),
        }
    }

    /// Changes the current working directory.
    pub fn chdir(&mut self, path: &Path) -> Result<()> {
        self.cwd_path = self.lookup_path(path, true)?;
        Ok(())
    }

    /// Changes the root directory (chroot).
    pub fn chroot(&mut self, path: &Path) -> Result<()> {
        let new_root = self.lookup_path(path, true)?;
        self.root_path = new_root.clone();
        self.cwd_path = new_root;
        Ok(())
    }

    pub fn cwd_path(&self) -> &PathComponent {
        &self.cwd_path
    }

    /// Resolves a path into an inode. If `follow_symlink` is `true`, symbolic
    /// links are resolved and will never return `INode::Symlink`.
    ///
    /// Fast path: avoids PathComponent/String heap allocations by walking
    /// the directory tree directly.  Falls back to `lookup_path` for paths
    /// containing ".." or symlinks in intermediate components.
    pub fn lookup_inode(&self, path: &Path, follow_symlink: bool) -> Result<INode> {
        if path.is_empty() {
            return Err(Error::new(Errno::ENOENT));
        }

        let start = if path.is_absolute() {
            self.root_path.inode.clone()
        } else {
            self.cwd_path.inode.clone()
        };

        let mut current = start;
        let mut components = path.components().peekable();
        while let Some(name) = components.next() {
            match name {
                "." => continue,
                ".." => {
                    // Fall back to full lookup for ".."
                    return self.lookup_path(path, follow_symlink)
                        .map(|pc| pc.inode.clone());
                }
                _ => {
                    let dir = match &current {
                        INode::Directory(d) => d,
                        _ => return Err(Error::new(Errno::ENOTDIR)),
                    };

                    let mut inode = dir.lookup(name)?;

                    // Check mount points.
                    if let INode::Directory(ref dir) = inode {
                        if let Some(mp) = self.lookup_mount_point(dir)? {
                            inode = mp.fs.root_dir()?.into();
                        }
                    }

                    if components.peek().is_some() {
                        // Intermediate component.
                        match &inode {
                            INode::Directory(_) => { current = inode; }
                            INode::Symlink(_) => {
                                // Fall back for symlinks in intermediate path.
                                return self.lookup_path(path, follow_symlink)
                                    .map(|pc| pc.inode.clone());
                            }
                            _ => return Err(Error::new(Errno::ENOTDIR)),
                        }
                    } else {
                        // Last component.
                        if follow_symlink {
                            if let INode::Symlink(symlink) = &inode {
                                let linked_to = symlink.linked_to()?;
                                return self.lookup_inode(Path::new(&*linked_to), follow_symlink);
                            }
                        }
                        return Ok(inode);
                    }
                }
            }
        }

        // Path is "/" or ends with "."
        Ok(current)
    }

    fn lookup_mount_point(&self, dir: &Arc<dyn Directory>) -> Result<Option<&MountPoint>> {
        let key = dir.mount_key()?;
        Ok(self.mount_points.get(&key))
    }

    /// Resolves a path into `PathComponent`. If `follow_symlink` is `true`,
    /// symbolic links are resolved and will never return `INode::Symlink`.
    pub fn lookup_path(&self, path: &Path, follow_symlink: bool) -> Result<Arc<PathComponent>> {
        let lookup_from = if path.is_absolute() {
            self.root_path.clone()
        } else {
            self.cwd_path.clone()
        };

        self.do_lookup_path(
            &lookup_from,
            path,
            follow_symlink,
            self.symlink_follow_limit,
        )
    }

    /// Resolves a path into `PathComponent` from the given directory `cwd_or_fd`.
    /// If `follow_symlink` is `true`, symbolic links are resolved and will
    /// never return `INode::Symlink`.
    pub fn lookup_path_at(
        &self,
        opened_files: &OpenedFileTable,
        cwd_or_fd: &CwdOrFd,
        path: &Path,
        follow_symlink: bool,
    ) -> Result<Arc<PathComponent>> {
        self.do_lookup_path(
            &self.resolve_cwd_or_fd(opened_files, cwd_or_fd, path)?,
            path,
            follow_symlink,
            self.symlink_follow_limit,
        )
    }

    pub fn lookup_parent_path_at<'a>(
        &self,
        opened_files: &OpenedFileTable,
        cwd_or_fd: &CwdOrFd,
        path: &'a Path,
        follow_symlink: bool,
    ) -> Result<(Arc<PathComponent>, &'a str)> {
        let (parent_dir, name) = path
            .parent_and_basename()
            .ok_or_else::<Error, _>(|| Errno::EEXIST.into())?;
        let path = self.lookup_path_at(opened_files, cwd_or_fd, parent_dir, follow_symlink)?;
        Ok((path, name))
    }

    fn resolve_cwd_or_fd(
        &self,
        opened_files: &OpenedFileTable,
        cwd_or_fd: &CwdOrFd,
        path: &Path,
    ) -> Result<Arc<PathComponent>> {
        if path.is_absolute() {
            Ok(self.root_path.clone())
        } else {
            match cwd_or_fd {
                CwdOrFd::AtCwd => Ok(self.cwd_path.clone()),
                CwdOrFd::Fd(fd) => {
                    let opened_file = opened_files.get(*fd)?;
                    Ok(opened_file.path().clone())
                }
            }
        }
    }

    fn do_lookup_path(
        &self,
        lookup_from: &Arc<PathComponent>,
        path: &Path,
        follow_symlink: bool,
        symlink_follow_limit: usize,
    ) -> Result<Arc<PathComponent>> {
        let _span = crate::debug::tracer::span_guard(crate::debug::tracer::span::PATH_LOOKUP);
        if path.is_empty() {
            return Err(Error::new(Errno::ENOENT));
        }

        let mut parent_dir = lookup_from.clone();

        // Iterate and resolve each component (e.g. `a`, `b`, and `c` in `a/b/c`).
        let mut components = path.components().peekable();
        while let Some(name) = components.next() {
            let path_comp = match name {
                // Handle some special cases that appear in a relative path.
                "." => continue,
                ".." => parent_dir
                    .parent_dir
                    .as_ref()
                    .unwrap_or(&self.root_path)
                    .clone(),
                // Look for the entry with the name in the directory.
                _ => {
                    let inode = match parent_dir.inode.as_dir()?.lookup(name)? {
                        // If it is a directory and it's a mount point, go
                        // into the mounted file system's root.
                        INode::Directory(dir) => match self.lookup_mount_point(&dir)? {
                            Some(mount_point) => mount_point.fs.root_dir()?.into(),
                            None => dir.into(),
                        },
                        inode => inode,
                    };

                    Arc::new(PathComponent {
                        parent_dir: Some(parent_dir.clone()),
                        name: name.to_owned(),
                        inode,
                    })
                }
            };

            if components.peek().is_some() {
                // Ancestor components: `a` and `b` in `a/b/c`. Visit the next
                // level directory.
                parent_dir = match &path_comp.inode {
                    INode::Directory(_) => path_comp,
                    INode::Symlink(symlink) => {
                        // Follow the symlink even if follow_symlinks is false since
                        // it's not the last one of the path components.

                        if symlink_follow_limit == 0 {
                            return Err(Errno::ELOOP.into());
                        }

                        let linked_to = symlink.linked_to()?;
                        let linked_path = Path::new(&*linked_to);
                        let follow_from = if linked_path.is_absolute() {
                            &self.root_path
                        } else {
                            &parent_dir
                        };

                        let dst_path = self.do_lookup_path(
                            follow_from,
                            linked_path,
                            follow_symlink,
                            symlink_follow_limit - 1,
                        )?;

                        // Check if the desitnation is a directory.
                        match &dst_path.inode {
                            INode::Directory(_) => dst_path,
                            _ => return Err(Errno::ENOTDIR.into()),
                        }
                    }
                    INode::FileLike(_) => {
                        // The next level must be an directory since the current component
                        // is not the last one.
                        return Err(Errno::ENOTDIR.into());
                    }
                }
            } else {
                // The last component: `c` in `a/b/c`.
                match &path_comp.inode {
                    INode::Symlink(symlink) if follow_symlink => {
                        if symlink_follow_limit == 0 {
                            return Err(Errno::ELOOP.into());
                        }

                        let linked_to = symlink.linked_to()?;
                        let linked_path = Path::new(&*linked_to);
                        let follow_from = if linked_path.is_absolute() {
                            &self.root_path
                        } else {
                            &parent_dir
                        };

                        return self.do_lookup_path(
                            follow_from,
                            linked_path,
                            follow_symlink,
                            symlink_follow_limit - 1,
                        );
                    }
                    _ => {
                        return Ok(path_comp);
                    }
                }
            }
        }

        // Here's reachable if the path points to the root (i.e. "/") or the path
        // ends with "." (e.g. "." and "a/b/c/.").
        Ok(parent_dir)
    }
}
