// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! In-memory temporary filesystem (tmpfs) for Kevlar.
//!
//! This is a Ring 2 service crate — it depends only on `kevlar_vfs` traits,
//! `kevlar_platform` (for `SpinLock`), and `kevlar_utils`.  It contains no
//! `unsafe` code.
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::{borrow::Cow, string::String, sync::Arc, vec::Vec};
use core::{
    fmt,
    sync::atomic::{AtomicU32, AtomicUsize, Ordering},
};
use hashbrown::HashMap;
use kevlar_platform::spinlock::SpinLock;
use kevlar_utils::{downcast::{downcast, Downcastable}, once::Once};

use kevlar_vfs::{
    file_system::FileSystem,
    inode::{
        DirEntry, Directory, FileLike, FileType, INode, INodeNo, OpenOptions,
        Symlink as SymlinkTrait,
    },
    result::{Errno, Error, Result},
    stat::{FileMode, GId, Stat, UId, S_IFDIR, S_IFLNK, S_IFMT, S_IFREG, S_IFSOCK},
    user_buffer::{UserBufReader, UserBufWriter, UserBuffer, UserBufferMut},
};

pub static TMP_FS: Once<Arc<TmpFs>> = Once::new();

/// Current wall-clock as (seconds, nanoseconds) from VFS clock.
fn now_ts() -> (u32, u32) {
    kevlar_vfs::vfs_clock_ts()
}

/// Current wall-clock seconds since epoch (from VFS clock).
fn now_secs() -> u32 {
    kevlar_vfs::vfs_clock_secs()
}

fn alloc_inode_no() -> INodeNo {
    // Inode #1 is reserved for the root dir.
    static NEXT_INODE_NO: AtomicUsize = AtomicUsize::new(2);

    INodeNo::new(NEXT_INODE_NO.fetch_add(1, Ordering::Relaxed))
}

pub struct TmpFs {
    root_dir: Arc<Dir>,
    #[allow(dead_code)]
    dev_id: usize,
}

impl TmpFs {
    pub fn new() -> TmpFs {
        let dev_id = kevlar_vfs::inode::alloc_dev_id();
        TmpFs {
            root_dir: Arc::new(Dir::new(INodeNo::new(1), dev_id)),
            dev_id,
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
    /// Ordered list of entry names for O(1) positional readdir access.
    /// Kept in sync with `files` by insert/remove operations.
    order: Vec<String>,
}

impl DirInner {
    fn insert(&mut self, name: String, inode: TmpFsINode) {
        if !self.files.contains_key(&name) {
            self.order.push(name.clone());
        }
        self.files.insert(name, inode);
    }

    fn remove(&mut self, name: &str) -> Option<TmpFsINode> {
        let result = self.files.remove(name);
        if result.is_some() {
            self.order.retain(|n| n != name);
        }
        result
    }
}

pub struct Dir {
    inode_no: INodeNo,
    dev_id: usize,
    stat: Stat,
    mode: SpinLock<FileMode>,
    uid: SpinLock<UId>,
    gid: SpinLock<GId>,
    inner: SpinLock<DirInner>,
    mtime: AtomicU32,
    ctime: AtomicU32,
}

impl Dir {
    pub fn new(inode_no: INodeNo, dev_id: usize) -> Dir {
        let mode = FileMode::new(S_IFDIR | 0o755);
        let now = now_secs();
        Dir {
            inode_no,
            dev_id,
            stat: Stat {
                inode_no,
                mode,
                ..Stat::zeroed()
            },
            mode: SpinLock::new(mode),
            uid: SpinLock::new(UId::new(0)),
            gid: SpinLock::new(GId::new(0)),
            inner: SpinLock::new(DirInner {
                files: HashMap::new(),
                order: Vec::new(),
            }),
            mtime: AtomicU32::new(now),
            ctime: AtomicU32::new(now),
        }
    }

    /// Update mtime/ctime to current time (called on directory modifications).
    fn touch(&self) {
        let now = now_secs();
        self.mtime.store(now, Ordering::Relaxed);
        self.ctime.store(now, Ordering::Relaxed);
    }

    pub fn add_dir(&self, name: &str) -> Arc<Dir> {
        let dir = Arc::new(Dir::new(alloc_inode_no(), self.dev_id));
        self.inner.lock_no_irq().insert(name.into(), TmpFsINode::Directory(dir.clone()));
        dir
    }

    /// Like `add_dir`, but if a directory of the same name already
    /// exists, return it instead of overwriting.  Useful for nested
    /// runtime additions like `/dev/dri/cardN` where multiple call
    /// sites create the subdir.  If a non-directory entry exists at
    /// the name, it is overwritten with a fresh directory.
    pub fn get_or_add_dir(&self, name: &str) -> Arc<Dir> {
        let mut inner = self.inner.lock_no_irq();
        if let Some(TmpFsINode::Directory(existing)) = inner.files.get(name) {
            return existing.clone();
        }
        let dir = Arc::new(Dir::new(alloc_inode_no(), self.dev_id));
        inner.insert(name.into(), TmpFsINode::Directory(dir.clone()));
        dir
    }

    pub fn add_file(&self, name: &str, file: Arc<dyn FileLike>) {
        self.inner.lock_no_irq().insert(name.into(), TmpFsINode::File(file));
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
        // Synthesize "." and ".." as the first two entries.
        if index == 0 {
            return Ok(Some(DirEntry {
                inode_no: self.inode_no,
                file_type: FileType::Directory,
                name: String::from("."),
            }));
        }
        if index == 1 {
            return Ok(Some(DirEntry {
                inode_no: self.inode_no, // parent not tracked; use self
                file_type: FileType::Directory,
                name: String::from(".."),
            }));
        }

        let dir_lock = self.inner.lock_no_irq();
        let name = match dir_lock.order.get(index - 2) {
            Some(n) => n,
            None => return Ok(None),
        };
        let inode = match dir_lock.files.get(name) {
            Some(i) => i,
            None => return Ok(None),
        };

        let entry = match inode {
            TmpFsINode::Directory(dir) => {
                DirEntry {
                    inode_no: dir.inode_no,
                    file_type: FileType::Directory,
                    name: name.clone(),
                }
            }
            TmpFsINode::File(file) => {
                let st = file.stat()?;
                let ft = if (st.mode.as_u32() & S_IFMT) == S_IFSOCK {
                    FileType::Socket
                } else {
                    FileType::Regular
                };
                DirEntry {
                    inode_no: st.inode_no,
                    file_type: ft,
                    name: name.clone(),
                }
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
        use kevlar_vfs::stat::Time;
        let mut st = self.stat;
        st.mode = *self.mode.lock_no_irq();
        st.uid = *self.uid.lock_no_irq();
        st.gid = *self.gid.lock_no_irq();
        let mt = self.mtime.load(Ordering::Relaxed) as isize;
        st.atime = Time::new(mt); // atime tracks mtime for simplicity
        st.mtime = Time::new(mt);
        st.ctime = Time::new(self.ctime.load(Ordering::Relaxed) as isize);
        Ok(st)
    }

    fn chmod(&self, mode: FileMode) -> Result<()> {
        let mut m = self.mode.lock_no_irq();
        let type_bits = m.as_u32() & 0o170000;
        *m = FileMode::new(type_bits | (mode.as_u32() & 0o7777));
        Ok(())
    }

    fn chown(&self, uid: UId, gid: GId) -> Result<()> {
        *self.uid.lock_no_irq() = uid;
        *self.gid.lock_no_irq() = gid;
        Ok(())
    }

    fn set_times(&self, atime_secs: Option<isize>, mtime_secs: Option<isize>) -> Result<()> {
        if let Some(m) = mtime_secs {
            self.mtime.store(m as u32, Ordering::Relaxed);
            self.ctime.store(m as u32, Ordering::Relaxed);
        }
        Ok(())
    }

    fn inode_no(&self) -> Result<INodeNo> {
        Ok(self.inode_no)
    }

    fn dev_id(&self) -> usize {
        self.dev_id
    }

    fn link(&self, name: &str, link_to: &INode) -> Result<()> {
        let tmpfs_inode = match link_to {
            INode::FileLike(file_like) => {
                // Increment nlink for tmpfs files. Deref through Arc to avoid
                // the Arc<dyn FileLike> downcast bug.
                if let Some(file) = (**file_like).as_any().downcast_ref::<File>() {
                    file.nlink.fetch_add(1, Ordering::Relaxed);
                }
                TmpFsINode::File(file_like.clone())
            }
            INode::Directory(dir) => {
                let dir: &Arc<Dir> = downcast(dir).unwrap();
                TmpFsINode::Directory(dir.clone())
            }
            INode::Symlink(sym) => {
                let sym: &Arc<TmpFsSymlink> = downcast(sym).unwrap();
                TmpFsINode::Symlink(sym.clone())
            }
        };

        self.inner.lock_no_irq().insert(name.into(), tmpfs_inode);
        Ok(())
    }

    fn create_file(&self, name: &str, mode: FileMode, uid: UId, gid: GId) -> Result<INode> {
        if name == "t" || name == "t.c" || name == "t.o" {
            kevlar_platform::println!("\x1b[31mtmpfs create: {:?}\x1b[0m", name);
        }
        let mut dir_lock = self.inner.lock_no_irq();
        if dir_lock.files.contains_key(name) {
            return Err(Errno::EEXIST.into());
        }

        let inode = Arc::new(File::new(alloc_inode_no()));
        // Apply the requested permission bits (umask already applied by caller).
        // Honor S_IFSOCK in the type bits — AF_UNIX bind() to a filesystem
        // path creates a socket node here so that subsequent stat() reports
        // S_IFSOCK and shell tests like `[ -S /path ]` succeed (matches
        // Linux semantics).  All other types fall back to S_IFREG.
        let resolved_type = match mode.as_u32() & S_IFMT {
            S_IFSOCK => S_IFSOCK,
            _ => S_IFREG,
        };
        *inode.mode.lock_no_irq() = FileMode::new(resolved_type | (mode.as_u32() & 0o7777));
        *inode.uid.lock_no_irq() = uid;
        *inode.gid.lock_no_irq() = gid;
        dir_lock.insert(name.into(), TmpFsINode::File(inode.clone()));
        drop(dir_lock);
        self.touch();

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
        dir_lock.insert(name.into(), TmpFsINode::Symlink(inode.clone()));
        drop(dir_lock);
        self.touch();
        Ok((inode as Arc<dyn SymlinkTrait>).into())
    }

    fn create_dir(&self, name: &str, mode: FileMode, uid: UId, gid: GId) -> Result<INode> {
        let mut dir_lock = self.inner.lock_no_irq();
        if dir_lock.files.contains_key(name) {
            return Err(Errno::EEXIST.into());
        }
        let inode = Arc::new(Dir::new(alloc_inode_no(), self.dev_id));
        // Apply requested permission bits (umask already applied by caller).
        *inode.mode.lock_no_irq() = FileMode::new(S_IFDIR | (mode.as_u32() & 0o7777));
        *inode.uid.lock_no_irq() = uid;
        *inode.gid.lock_no_irq() = gid;
        dir_lock.insert(name.into(), TmpFsINode::Directory(inode.clone()));
        drop(dir_lock);
        self.touch();
        Ok((inode as Arc<dyn Directory>).into())
    }

    fn unlink(&self, name: &str) -> Result<()> {
        let mut dir_lock = self.inner.lock_no_irq();
        match dir_lock.files.get(name) {
            Some(TmpFsINode::Directory(_)) => return Err(Errno::EISDIR.into()),
            Some(TmpFsINode::File(_)) | Some(TmpFsINode::Symlink(_)) => {}
            None => return Err(Errno::ENOENT.into()),
        }
        if let Some(TmpFsINode::File(file_like)) = dir_lock.files.get(name) {
            if let Some(file) = (**file_like).as_any().downcast_ref::<File>() {
                file.nlink.fetch_sub(1, Ordering::Relaxed);
            }
        }
        dir_lock.remove(name);
        drop(dir_lock);
        self.touch();
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
        dir_lock.remove(name);
        drop(dir_lock);
        self.touch();
        Ok(())
    }

    fn rename(&self, old_name: &str, new_dir: &Arc<dyn Directory>, new_name: &str) -> Result<()> {
        // Deref through Arc to dispatch via vtable (avoids Arc<dyn> downcast bug).
        let new_dir: &Dir = (**new_dir).as_any().downcast_ref::<Dir>()
            .ok_or_else(|| Error::new(Errno::EXDEV))?;
        let self_ptr = self as *const Dir as usize;
        let new_ptr = new_dir as *const Dir as usize;

        // Handle same-directory rename without deadlock.
        if self_ptr == new_ptr {
            let mut dir_lock = self.inner.lock_no_irq();
            let entry = dir_lock.remove(old_name)
                .ok_or_else(|| Error::new(Errno::ENOENT))?;
            dir_lock.insert(new_name.into(), entry);
            return Ok(());
        }

        // Cross-directory: lock in pointer order to avoid deadlock.
        if self_ptr < new_ptr {
            let mut old_lock = self.inner.lock_no_irq();
            let mut new_lock = new_dir.inner.lock_no_irq();
            let entry = old_lock.remove(old_name)
                .ok_or_else(|| Error::new(Errno::ENOENT))?;
            new_lock.insert(new_name.into(), entry);
        } else {
            let mut new_lock = new_dir.inner.lock_no_irq();
            let mut old_lock = self.inner.lock_no_irq();
            let entry = old_lock.remove(old_name)
                .ok_or_else(|| Error::new(Errno::ENOENT))?;
            new_lock.insert(new_name.into(), entry);
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
    mode: SpinLock<FileMode>,
    uid: SpinLock<UId>,
    gid: SpinLock<GId>,
    nlink: AtomicUsize,
    atime: AtomicU32,
    mtime: AtomicU32,
    ctime: AtomicU32,
    mtime_nsec: AtomicU32,
}

impl File {
    pub fn new(inode_no: INodeNo) -> File {
        let mode = FileMode::new(S_IFREG | 0o644);
        let (secs, nsec) = now_ts();
        File {
            data: SpinLock::new(Vec::new()),
            stat: Stat {
                inode_no,
                mode,
                ..Stat::zeroed()
            },
            mode: SpinLock::new(mode),
            uid: SpinLock::new(UId::new(0)),
            gid: SpinLock::new(GId::new(0)),
            nlink: AtomicUsize::new(1),
            atime: AtomicU32::new(secs),
            mtime: AtomicU32::new(secs),
            ctime: AtomicU32::new(secs),
            mtime_nsec: AtomicU32::new(nsec),
        }
    }
}

impl FileLike for File {
    fn inode_key(&self) -> Result<(usize, u64)> {
        Ok((self.stat.dev.as_usize(), self.stat.inode_no.as_u64()))
    }

    fn stat(&self) -> Result<Stat> {
        use kevlar_vfs::stat::{FileSize, NLink, Time};
        let mut stat = self.stat;
        stat.mode = *self.mode.lock_no_irq();
        stat.uid = *self.uid.lock_no_irq();
        stat.gid = *self.gid.lock_no_irq();
        stat.size = FileSize(self.data.lock_no_irq().len() as isize);
        stat.nlink = NLink::new(self.nlink.load(Ordering::Relaxed));
        stat.atime = Time::new(self.atime.load(Ordering::Relaxed) as isize);
        stat.mtime = Time::new(self.mtime.load(Ordering::Relaxed) as isize);
        stat.ctime = Time::new(self.ctime.load(Ordering::Relaxed) as isize);
        let nsec = self.mtime_nsec.load(Ordering::Relaxed) as isize;
        stat.atime_nsec = Time::new(nsec);
        stat.mtime_nsec = Time::new(nsec);
        stat.ctime_nsec = Time::new(nsec);
        Ok(stat)
    }

    fn set_times(&self, atime_secs: Option<isize>, mtime_secs: Option<isize>) -> Result<()> {
        if let Some(a) = atime_secs {
            self.atime.store(a as u32, Ordering::Relaxed);
        }
        if let Some(m) = mtime_secs {
            self.mtime.store(m as u32, Ordering::Relaxed);
            self.ctime.store(m as u32, Ordering::Relaxed);
        }
        Ok(())
    }

    fn chmod(&self, mode: FileMode) -> Result<()> {
        let mut m = self.mode.lock_no_irq();
        let type_bits = m.as_u32() & 0o170000;
        *m = FileMode::new(type_bits | (mode.as_u32() & 0o7777));
        Ok(())
    }

    fn chown(&self, uid: UId, gid: GId) -> Result<()> {
        *self.uid.lock_no_irq() = uid;
        *self.gid.lock_no_irq() = gid;
        Ok(())
    }

    fn read(&self, offset: usize, buf: UserBufferMut<'_>, _options: &OpenOptions) -> Result<usize> {
        let data = self.data.lock_no_irq();
        if offset > data.len() {
            return Ok(0);
        }

        let available = &data[offset..];
        let copy_len = core::cmp::min(available.len(), buf.len());

        // For small reads (≤ PAGE_SIZE), copy to a stack buffer under the lock
        // then release the lock before the usercopy. This reduces lock hold time
        // from usercopy duration to a fast memcpy.
        if copy_len <= 4096 {
            let mut tmp = [0u8; 4096];
            tmp[..copy_len].copy_from_slice(&available[..copy_len]);
            drop(data);
            let mut writer = UserBufWriter::from(buf);
            writer.write_bytes(&tmp[..copy_len])
        } else {
            let mut writer = UserBufWriter::from(buf);
            writer.write_bytes(available)
        }
    }

    fn write(
        &self,
        offset: usize,
        buf: UserBuffer<'_>,
        _options: &OpenOptions,
    ) -> Result<usize> {
        let mut data = self.data.lock_no_irq();
        let mut reader = UserBufReader::from(buf);
        let new_len = offset + reader.remaining_len();
        if new_len > data.len() {
            let cap = data.capacity();
            if new_len > cap {
                data.reserve_exact(new_len - cap);
            }
            data.resize(new_len, 0);
        }
        let written = reader.read_bytes(&mut data[offset..])?;
        // Update mtime/ctime on successful write.
        let (secs, nsec) = now_ts();
        self.mtime.store(secs, Ordering::Relaxed);
        self.ctime.store(secs, Ordering::Relaxed);
        self.mtime_nsec.store(nsec, Ordering::Relaxed);
        Ok(written)
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

    fn linked_to(&self) -> Result<Cow<'_, str>> {
        Ok(Cow::Borrowed(&self.target))
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
