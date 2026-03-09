// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! In-memory temporary filesystem (tmpfs) for Kevlar.
//!
//! This is a Ring 2 service crate — it depends only on `kevlar_vfs` traits,
//! `kevlar_platform` (for `SpinLock`), and `kevlar_utils`.  It contains no
//! `unsafe` code.
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::{string::String, sync::Arc, vec::Vec};
use core::{
    fmt,
    sync::atomic::{AtomicUsize, Ordering},
};
use hashbrown::HashMap;
use kevlar_platform::spinlock::SpinLock;
use kevlar_utils::{downcast::downcast, once::Once};

use kevlar_vfs::{
    file_system::FileSystem,
    inode::{
        DirEntry, Directory, FileLike, FileType, INode, INodeNo, OpenOptions,
        Symlink as SymlinkTrait,
    },
    path::PathBuf,
    result::{Errno, Error, Result},
    stat::{FileMode, Stat, S_IFDIR, S_IFLNK, S_IFREG},
    user_buffer::{UserBufReader, UserBufWriter, UserBuffer, UserBufferMut},
};

pub static TMP_FS: Once<Arc<TmpFs>> = Once::new();

fn alloc_inode_no() -> INodeNo {
    // Inode #1 is reserved for the root dir.
    static NEXT_INODE_NO: AtomicUsize = AtomicUsize::new(2);

    INodeNo::new(NEXT_INODE_NO.fetch_add(1, Ordering::Relaxed))
}

pub struct TmpFs {
    root_dir: Arc<Dir>,
}

impl TmpFs {
    pub fn new() -> TmpFs {
        TmpFs {
            root_dir: Arc::new(Dir::new(INodeNo::new(1))),
        }
    }

    pub fn root_tmpfs_dir(&self) -> &Arc<Dir> {
        &self.root_dir
    }
}

impl Default for TmpFs {
    fn default() -> Self {
        Self::new()
    }
}

impl FileSystem for TmpFs {
    fn root_dir(&self) -> Result<Arc<dyn Directory>> {
        Ok(self.root_dir.clone())
    }
}

enum TmpFsINode {
    File(Arc<dyn FileLike>),
    Directory(Arc<Dir>),
    Symlink(Arc<TmpFsSymlink>),
}

struct DirInner {
    files: HashMap<String, TmpFsINode>,
}

pub struct Dir {
    inode_no: INodeNo,
    stat: Stat,
    inner: SpinLock<DirInner>,
}

impl Dir {
    pub fn new(inode_no: INodeNo) -> Dir {
        Dir {
            inode_no,
            stat: Stat {
                inode_no,
                mode: FileMode::new(S_IFDIR | 0o755),
                ..Stat::zeroed()
            },
            inner: SpinLock::new(DirInner {
                files: HashMap::new(),
            }),
        }
    }

    pub fn add_dir(&self, name: &str) -> Arc<Dir> {
        let dir = Arc::new(Dir::new(alloc_inode_no()));
        self.inner
            .lock_no_irq()
            .files
            .insert(name.into(), TmpFsINode::Directory(dir.clone()));
        dir
    }

    pub fn add_file(&self, name: &str, file: Arc<dyn FileLike>) {
        self.inner
            .lock_no_irq()
            .files
            .insert(name.into(), TmpFsINode::File(file));
    }
}

impl Directory for Dir {
    fn lookup(&self, name: &str) -> Result<INode> {
        self.inner
            .lock_no_irq()
            .files
            .get(name)
            .map(|tmpfs_inode| match tmpfs_inode {
                TmpFsINode::File(file) => file.clone().into(),
                TmpFsINode::Directory(dir) => (dir.clone() as Arc<dyn Directory>).into(),
                TmpFsINode::Symlink(sym) => (sym.clone() as Arc<dyn SymlinkTrait>).into(),
            })
            .ok_or_else(|| Error::new(Errno::ENOENT))
    }

    fn readdir(&self, index: usize) -> Result<Option<DirEntry>> {
        let dir_lock = self.inner.lock_no_irq();
        let (name, inode) = match dir_lock.files.iter().nth(index) {
            Some(entry) => entry,
            None => {
                return Ok(None);
            }
        };

        let entry = match inode {
            TmpFsINode::Directory(dir) => {
                DirEntry {
                    inode_no: dir.inode_no,
                    file_type: FileType::Directory,
                    name: name.clone(),
                }
            }
            TmpFsINode::File(file) => DirEntry {
                inode_no: file.stat()?.inode_no,
                file_type: FileType::Regular,
                name: name.clone(),
            },
            TmpFsINode::Symlink(sym) => DirEntry {
                inode_no: sym.stat.inode_no,
                file_type: FileType::Link,
                name: name.clone(),
            },
        };

        Ok(Some(entry))
    }

    fn stat(&self) -> Result<Stat> {
        Ok(self.stat)
    }

    fn inode_no(&self) -> Result<INodeNo> {
        Ok(self.inode_no)
    }

    fn link(&self, name: &str, link_to: &INode) -> Result<()> {
        let tmpfs_inode = match link_to {
            INode::FileLike(file_like) => TmpFsINode::File(file_like.clone()),
            INode::Directory(dir) => {
                let dir: &Arc<Dir> = downcast(dir).unwrap();
                TmpFsINode::Directory(dir.clone())
            }
            INode::Symlink(sym) => {
                let sym: &Arc<TmpFsSymlink> = downcast(sym).unwrap();
                TmpFsINode::Symlink(sym.clone())
            }
        };

        self.inner.lock_no_irq().files.insert(name.into(), tmpfs_inode);
        Ok(())
    }

    fn create_file(&self, name: &str, _mode: FileMode) -> Result<INode> {
        let mut dir_lock = self.inner.lock_no_irq();
        if dir_lock.files.contains_key(name) {
            return Err(Errno::EEXIST.into());
        }

        let inode = Arc::new(File::new(alloc_inode_no()));
        dir_lock
            .files
            .insert(name.into(), TmpFsINode::File(inode.clone()));

        Ok((inode as Arc<dyn FileLike>).into())
    }

    fn create_symlink(&self, name: &str, target: &str) -> Result<INode> {
        let mut dir_lock = self.inner.lock_no_irq();
        if dir_lock.files.contains_key(name) {
            return Err(Errno::EEXIST.into());
        }

        let inode = Arc::new(TmpFsSymlink {
            target: target.into(),
            stat: Stat {
                inode_no: alloc_inode_no(),
                mode: FileMode::new(S_IFLNK | 0o777),
                ..Stat::zeroed()
            },
        });
        dir_lock
            .files
            .insert(name.into(), TmpFsINode::Symlink(inode.clone()));
        Ok((inode as Arc<dyn SymlinkTrait>).into())
    }

    fn create_dir(&self, name: &str, _mode: FileMode) -> Result<INode> {
        let inode = Arc::new(Dir::new(alloc_inode_no()));
        self.inner
            .lock_no_irq()
            .files
            .insert(name.into(), TmpFsINode::Directory(inode.clone()));

        Ok((inode as Arc<dyn Directory>).into())
    }

    fn unlink(&self, name: &str) -> Result<()> {
        let mut dir_lock = self.inner.lock_no_irq();
        match dir_lock.files.get(name) {
            Some(TmpFsINode::Directory(_)) => return Err(Errno::EISDIR.into()),
            Some(TmpFsINode::File(_)) | Some(TmpFsINode::Symlink(_)) => {}
            None => return Err(Errno::ENOENT.into()),
        }
        dir_lock.files.remove(name);
        Ok(())
    }

    fn rmdir(&self, name: &str) -> Result<()> {
        let mut dir_lock = self.inner.lock_no_irq();
        match dir_lock.files.get(name) {
            Some(TmpFsINode::Directory(dir)) => {
                if !dir.inner.lock_no_irq().files.is_empty() {
                    return Err(Errno::ENOTEMPTY.into());
                }
            }
            Some(TmpFsINode::File(_)) | Some(TmpFsINode::Symlink(_)) => {
                return Err(Errno::ENOTDIR.into())
            }
            None => return Err(Errno::ENOENT.into()),
        }
        dir_lock.files.remove(name);
        Ok(())
    }

    fn rename(&self, old_name: &str, new_dir: &Arc<dyn Directory>, new_name: &str) -> Result<()> {
        let new_dir: &Arc<Dir> = downcast(new_dir).ok_or_else(|| Error::new(Errno::EXDEV))?;
        let self_ptr = self as *const Dir as usize;
        let new_ptr = &**new_dir as *const Dir as usize;

        // Handle same-directory rename without deadlock.
        if self_ptr == new_ptr {
            let mut dir_lock = self.inner.lock_no_irq();
            let entry = dir_lock
                .files
                .remove(old_name)
                .ok_or_else(|| Error::new(Errno::ENOENT))?;
            dir_lock.files.insert(new_name.into(), entry);
            return Ok(());
        }

        // Cross-directory: lock in pointer order to avoid deadlock.
        if self_ptr < new_ptr {
            let mut old_lock = self.inner.lock_no_irq();
            let mut new_lock = new_dir.inner.lock_no_irq();
            let entry = old_lock
                .files
                .remove(old_name)
                .ok_or_else(|| Error::new(Errno::ENOENT))?;
            new_lock.files.insert(new_name.into(), entry);
        } else {
            let mut new_lock = new_dir.inner.lock_no_irq();
            let mut old_lock = self.inner.lock_no_irq();
            let entry = old_lock
                .files
                .remove(old_name)
                .ok_or_else(|| Error::new(Errno::ENOENT))?;
            new_lock.files.insert(new_name.into(), entry);
        }
        Ok(())
    }
}

impl fmt::Debug for Dir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TmpFsDir").finish()
    }
}

struct File {
    data: SpinLock<Vec<u8>>,
    stat: Stat,
}

impl File {
    pub fn new(inode_no: INodeNo) -> File {
        File {
            data: SpinLock::new(Vec::new()),
            stat: Stat {
                inode_no,
                mode: FileMode::new(S_IFREG | 0o644),
                ..Stat::zeroed()
            },
        }
    }
}

impl FileLike for File {
    fn stat(&self) -> Result<Stat> {
        use kevlar_vfs::stat::FileSize;
        let mut stat = self.stat;
        stat.size = FileSize(self.data.lock_no_irq().len() as isize);
        Ok(stat)
    }

    fn read(&self, offset: usize, buf: UserBufferMut<'_>, _options: &OpenOptions) -> Result<usize> {
        let data = self.data.lock_no_irq();
        if offset > data.len() {
            return Ok(0);
        }

        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&data[offset..])
    }

    fn write(
        &self,
        offset: usize,
        buf: UserBuffer<'_>,
        _options: &OpenOptions,
    ) -> Result<usize> {
        let mut data = self.data.lock_no_irq();
        let mut reader = UserBufReader::from(buf);
        data.resize(offset + reader.remaining_len(), 0);
        reader.read_bytes(&mut data[offset..])
    }

    fn truncate(&self, length: usize) -> Result<()> {
        self.data.lock_no_irq().resize(length, 0);
        Ok(())
    }
}

impl fmt::Debug for File {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TmpFsFile").finish()
    }
}

struct TmpFsSymlink {
    target: String,
    stat: Stat,
}

impl SymlinkTrait for TmpFsSymlink {
    fn stat(&self) -> Result<Stat> {
        Ok(self.stat)
    }

    fn linked_to(&self) -> Result<PathBuf> {
        Ok(PathBuf::from(self.target.clone()))
    }
}

impl fmt::Debug for TmpFsSymlink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TmpFsSymlink")
            .field("target", &self.target)
            .finish()
    }
}

pub fn init() {
    TMP_FS.init(|| Arc::new(TmpFs::new()));
}
