// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use core::fmt::{self, Debug};
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::result::{Errno, Error, Result};
use crate::socket_types::{RecvFromFlags, ShutdownHow, SockAddr};
use crate::stat::{FileMode, GId, Stat, UId};
use crate::user_buffer::{UserBuffer, UserBufferMut};
use alloc::borrow::Cow;
use alloc::string::String;
use alloc::sync::Arc;
use bitflags::bitflags;
use kevlar_utils::downcast::Downcastable;

/// Allocate a unique filesystem device ID.
/// Each filesystem instance should call this once to get a unique ID,
/// preventing inode number collisions in the mount point table.
pub fn alloc_dev_id() -> usize {
    static NEXT_DEV_ID: AtomicUsize = AtomicUsize::new(1);
    NEXT_DEV_ID.fetch_add(1, Ordering::Relaxed)
}

/// The inode number.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
#[repr(transparent)]
pub struct INodeNo(usize);

impl INodeNo {
    pub const fn new(no: usize) -> INodeNo {
        INodeNo(no)
    }

    pub const fn as_u64(self) -> u64 {
        self.0 as u64
    }
}

/// A unique mount key combining filesystem device ID and inode number.
/// This prevents inode number collisions across different filesystems
/// in the mount point table.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct MountKey {
    pub dev_id: usize,
    pub inode_no: INodeNo,
}

impl MountKey {
    pub fn new(dev_id: usize, inode_no: INodeNo) -> Self {
        Self { dev_id, inode_no }
    }
}

/// Options for opened files.
#[derive(Debug, Copy, Clone)]
pub struct OpenOptions {
    pub nonblock: bool,
    pub close_on_exec: bool,
    pub append: bool,
    /// Access mode bits: O_RDONLY=0, O_WRONLY=1, O_RDWR=2.
    pub access_mode: i32,
    /// O_SYNC / O_DSYNC: flush data to disk after each write.
    pub sync: bool,
}

impl OpenOptions {
    pub fn new(nonblock: bool, cloexec: bool) -> OpenOptions {
        OpenOptions {
            nonblock,
            close_on_exec: cloexec,
            append: false,
            access_mode: 0,
            sync: false,
        }
    }

    pub fn empty() -> OpenOptions {
        OpenOptions {
            nonblock: false,
            close_on_exec: false,
            append: false,
            access_mode: 0,
            sync: false,
        }
    }

    pub fn readwrite() -> OpenOptions {
        OpenOptions {
            nonblock: false,
            close_on_exec: false,
            append: false,
            access_mode: 2, // O_RDWR
            sync: false,
        }
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct PollStatus: i16 {
        const POLLIN     = 0x001;
        const POLLPRI    = 0x002;
        const POLLOUT    = 0x004;
        const POLLERR    = 0x008;
        const POLLHUP    = 0x010;
        const POLLNVAL   = 0x020;
        const POLLRDNORM = 0x040;
        const POLLRDBAND = 0x080;
        const POLLWRNORM = 0x100;
        const POLLWRBAND = 0x200;
    }
}

/// A file-like object (Ring 2 service boundary).
///
/// This trait represents an object which behaves like a file such as files on
/// disks (aka. regular files), UDP/TCP sockets, device files like tty, etc.
///
/// # Ringkernel notes
///
/// This trait is the primary Ring 2 service boundary for I/O. Filesystem and
/// network implementations provide concrete types implementing this trait.
/// In Phase 4, Core calls into `FileLike` methods will be wrapped in
/// `catch_unwind` for panic containment.
///
/// Methods below `fsync` are **socket-specific** (bind, listen, connect, etc.)
/// and will move to a separate `SocketOps` trait in Phase 3 when the network
/// stack is extracted into its own service crate.
pub trait FileLike: Debug + Send + Sync + Downcastable {
    /// `open(2)`.
    fn open(&self, _options: &OpenOptions) -> Result<Option<Arc<dyn FileLike>>> {
        Ok(None)
    }

    /// `stat(2)`.
    fn stat(&self) -> Result<Stat> {
        Err(Error::new(Errno::EBADF))
    }

    /// `readlink(2)`.
    fn readlink(&self) -> Result<Cow<'_, str>> {
        // "EINVAL - The named file is not a symbolic link." -- readlink(2)
        Err(Error::new(Errno::EINVAL))
    }

    /// `poll(2)` and `select(2)`.
    /// Default: regular files are always ready for I/O (matches Linux behavior).
    fn poll(&self) -> Result<PollStatus> {
        // Default: no events pending. Specific file types (pipes, sockets,
        // timerfd, signalfd) override this when they have data available.
        // This prevents epoll spin loops on procfs/regular files.
        Ok(PollStatus::empty())
    }

    /// Generation counter for edge-triggered epoll.  Incremented on every
    /// state change (read, write, close).  Returns 0 if not implemented
    /// (default), which makes ET fall back to level-triggered behavior.
    fn poll_gen(&self) -> u64 {
        0
    }

    /// Direct pointer to the poll_gen atomic, avoiding vtable dispatch in
    /// epoll's hot path. Returns None if poll_gen is not implemented.
    fn poll_gen_atomic(&self) -> Option<&core::sync::atomic::AtomicU64> {
        None
    }

    /// Notify the file that an EPOLLET watcher was added or removed.
    /// Used to skip expensive state_gen atomic RMW on the fast path
    /// when no edge-triggered watchers exist.
    fn notify_epoll_et(&self, _added: bool) {}

    /// Whether this file supports lseek. Pipes, sockets, and eventfds return
    /// false (lseek returns ESPIPE).
    fn is_seekable(&self) -> bool {
        true
    }

    /// For sockets: return the socket type (SOCK_STREAM=1, SOCK_DGRAM=2).
    /// Non-socket files return 0.
    fn socket_type(&self) -> i32 {
        0
    }

    /// `ioctl(2)`.
    fn ioctl(&self, _cmd: usize, _arg: usize) -> Result<isize> {
        Err(Error::new(Errno::ENOTTY))
    }

    /// `read(2)`.
    fn read(
        &self,
        _offset: usize,
        _buf: UserBufferMut<'_>,
        _options: &OpenOptions,
    ) -> Result<usize> {
        Err(Error::new(Errno::EBADF))
    }

    /// `write(2)`.
    fn write(&self, _offset: usize, _buf: UserBuffer<'_>, _options: &OpenOptions) -> Result<usize> {
        Err(Error::new(Errno::EBADF))
    }

    /// `ftruncate(2)`.
    fn truncate(&self, _length: usize) -> Result<()> {
        Err(Error::new(Errno::EINVAL))
    }

    /// `fallocate(2)`. Preallocate disk space for a file.
    /// Default: accept without action (tmpfs, devfs, etc.).
    /// Ext2/ext4 overrides to actually allocate blocks.
    fn fallocate(&self, _offset: usize, _len: usize) -> Result<()> {
        Ok(())
    }

    /// `fchmod(2)`.
    fn chmod(&self, _mode: FileMode) -> Result<()> {
        Ok(()) // silently succeed for filesystems that don't support it
    }

    /// `fchown(2)`.
    fn chown(&self, _uid: UId, _gid: GId) -> Result<()> {
        Ok(()) // silently succeed for filesystems that don't support it
    }

    /// Set file timestamps (seconds since epoch).
    /// `None` means "don't change this timestamp".
    fn set_times(&self, _atime_secs: Option<isize>, _mtime_secs: Option<isize>) -> Result<()> {
        Ok(()) // silently succeed for filesystems that don't support it
    }

    /// `fsync(2)`.
    fn fsync(&self) -> Result<()> {
        Ok(())
    }

    /// Called when the last fd referencing this file is closed.
    /// Filesystems should flush dirty data here.
    fn close(&self) -> Result<()> {
        Ok(())
    }

    /// Whether this file's content is immutable and demand-paged pages can be
    /// cached across processes (e.g., initramfs files backed by `&'static [u8]`).
    /// Default: false.
    fn is_content_immutable(&self) -> bool {
        false
    }

    /// Returns the kernel virtual address of the file's backing data, if it
    /// is `&'static [u8]` (e.g. initramfs). The kernel can convert this to a
    /// physical address for zero-copy mapping of page-aligned interior pages.
    fn data_vaddr(&self) -> Option<usize> {
        None
    }

    // --- Socket-specific methods (Phase 3: move to SocketOps trait) ---

    /// `bind(2)`.
    fn bind(&self, _sockaddr: SockAddr) -> Result<()> {
        Err(Error::new(Errno::EBADF))
    }

    /// `shutdown(2)`.
    fn shutdown(&self, _how: ShutdownHow) -> Result<()> {
        Err(Error::new(Errno::EBADF))
    }

    /// `listen(2)`.
    fn listen(&self, _backlog: i32) -> Result<()> {
        Err(Error::new(Errno::EBADF))
    }

    /// `getsockname(2)`.
    fn getsockname(&self) -> Result<SockAddr> {
        Err(Error::new(Errno::EBADF))
    }

    /// `getpeername(2)`.
    fn getpeername(&self) -> Result<SockAddr> {
        Err(Error::new(Errno::EBADF))
    }

    /// `accept(2)`.
    fn accept(&self, _options: &OpenOptions) -> Result<(Arc<dyn FileLike>, SockAddr)> {
        Err(Error::new(Errno::EBADF))
    }

    /// `connect(2)`.
    fn connect(&self, _sockaddr: SockAddr, _options: &OpenOptions) -> Result<()> {
        Err(Error::new(Errno::EBADF))
    }

    /// `sendto(2)`.
    fn sendto(
        &self,
        _buf: UserBuffer<'_>,
        _sockaddr: Option<SockAddr>,
        _options: &OpenOptions,
    ) -> Result<usize> {
        Err(Error::new(Errno::EBADF))
    }

    /// `recvfrom(2)`.
    fn recvfrom(
        &self,
        _buf: UserBufferMut<'_>,
        _flags: RecvFromFlags,
        _options: &OpenOptions,
    ) -> Result<(usize, SockAddr)> {
        Err(Error::new(Errno::EBADF))
    }
}

/// Represents `d_type` in `linux_dirent`. See `getdents64(2)` manual.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
#[repr(u8)]
#[non_exhaustive]
pub enum FileType {
    Directory = 4,
    Regular = 8,
    Link = 10,
}

/// A directory entry (ones returned from `readdir(3)`).
pub struct DirEntry {
    pub inode_no: INodeNo,
    pub file_type: FileType,
    pub name: String,
}

/// Represents a directory (Ring 2 service boundary).
///
/// Filesystem services implement this trait. In Phase 4, Core calls into
/// `Directory` methods will be wrapped in `catch_unwind` for panic containment.
pub trait Directory: Debug + Send + Sync + Downcastable {
    /// Looks for an existing file.
    fn lookup(&self, name: &str) -> Result<INode>;
    /// Creates a file. Returns `EEXIST` if the it already exists.
    fn create_file(&self, _name: &str, _mode: FileMode, _uid: UId, _gid: GId) -> Result<INode>;
    /// Creates a directory. Returns `EEXIST` if the it already exists.
    fn create_dir(&self, _name: &str, _mode: FileMode, _uid: UId, _gid: GId) -> Result<INode>;
    /// `stat(2)`.
    fn stat(&self) -> Result<Stat>;
    /// Returns the inode number without acquiring locks.
    /// Default implementation calls stat(), but filesystems can override
    /// for lock-free access when inode_no is immutable.
    fn inode_no(&self) -> Result<INodeNo> {
        self.stat().map(|s| s.inode_no)
    }
    /// Returns a filesystem-unique device ID. Each filesystem instance
    /// should return a unique value so that mount points keyed by
    /// (dev_id, inode_no) don't collide across filesystems.
    fn dev_id(&self) -> usize {
        0
    }
    /// Returns the mount key for this directory.
    fn mount_key(&self) -> Result<MountKey> {
        Ok(MountKey::new(self.dev_id(), self.inode_no()?))
    }
    /// `readdir(2)`.
    fn readdir(&self, index: usize) -> Result<Option<DirEntry>>;
    /// `link(2)`.
    fn link(&self, _name: &str, _link_to: &INode) -> Result<()>;
    /// `symlink(2)` — create a symbolic link in this directory.
    fn create_symlink(&self, _name: &str, _target: &str) -> Result<INode> {
        Err(Error::new(Errno::ENOSYS))
    }
    /// `unlink(2)` — remove a file entry from this directory.
    fn unlink(&self, _name: &str) -> Result<()> {
        Err(Error::new(Errno::ENOSYS))
    }
    /// `rmdir(2)` — remove a subdirectory entry from this directory.
    fn rmdir(&self, _name: &str) -> Result<()> {
        Err(Error::new(Errno::ENOSYS))
    }
    /// `rename(2)` — move an entry from this directory to another.
    fn rename(&self, _old_name: &str, _new_dir: &Arc<dyn Directory>, _new_name: &str) -> Result<()> {
        Err(Error::new(Errno::ENOSYS))
    }
    /// `fchmod(2)`.
    fn chmod(&self, _mode: FileMode) -> Result<()> {
        Ok(()) // silently succeed for filesystems that don't support it
    }
    /// `fchown(2)`.
    fn chown(&self, _uid: UId, _gid: GId) -> Result<()> {
        Ok(()) // silently succeed for filesystems that don't support it
    }
    /// Set directory timestamps (seconds since epoch).
    fn set_times(&self, _atime_secs: Option<isize>, _mtime_secs: Option<isize>) -> Result<()> {
        Ok(())
    }
    /// `fsync(2)`.
    fn fsync(&self) -> Result<()> {
        Ok(())
    }
    /// `readlink(2)`.
    fn readlink(&self) -> Result<Cow<'_, str>> {
        // "EINVAL - The named file is not a symbolic link." -- readlink(2)
        Err(Error::new(Errno::EINVAL))
    }
}

/// A symbolic link.
pub trait Symlink: Debug + Send + Sync + Downcastable {
    /// `stat(2)`.
    fn stat(&self) -> Result<Stat>;
    /// The path linked to.
    fn linked_to(&self) -> Result<Cow<'_, str>>;
    /// Set symlink timestamps (seconds since epoch).
    fn set_times(&self, _atime_secs: Option<isize>, _mtime_secs: Option<isize>) -> Result<()> {
        Ok(())
    }
    /// `fsync(2)`.
    fn fsync(&self) -> Result<()> {
        Ok(())
    }
}

/// An inode object.
#[derive(Clone)]
pub enum INode {
    FileLike(Arc<dyn FileLike>),
    Directory(Arc<dyn Directory>),
    Symlink(Arc<dyn Symlink>),
}

impl INode {
    /// Unwraps as a file. If it's not, returns `Errno::EBADF`.
    pub fn as_file(&self) -> Result<&Arc<dyn FileLike>> {
        match self {
            INode::FileLike(file) => Ok(file),
            _ => Err(Error::new(Errno::EBADF)),
        }
    }

    /// Unwraps as a directory. If it's not, returns `Errno::EBADF`.
    pub fn as_dir(&self) -> Result<&Arc<dyn Directory>> {
        match self {
            INode::Directory(dir) => Ok(dir),
            _ => Err(Error::new(Errno::EBADF)),
        }
    }

    /// Returns `true` if it's a file.
    pub fn is_file(&self) -> bool {
        matches!(self, INode::FileLike(_))
    }

    /// Returns `true` if it's a directory.
    pub fn is_dir(&self) -> bool {
        matches!(self, INode::Directory(_))
    }

    /// Whether this inode supports lseek (false for pipes, sockets, etc.).
    /// Directories are seekable on Linux (used by telldir/seekdir).
    pub fn is_seekable(&self) -> bool {
        match self {
            INode::FileLike(file) => file.is_seekable(),
            INode::Directory(_) => true,
            INode::Symlink(_) => false,
        }
    }

    /// `stat(2)`.
    pub fn stat(&self) -> Result<Stat> {
        match self {
            INode::FileLike(file) => file.stat(),
            INode::Symlink(file) => file.stat(),
            INode::Directory(dir) => dir.stat(),
        }
    }

    /// `fsync(2)`.
    pub fn fsync(&self) -> Result<()> {
        match self {
            INode::FileLike(file) => file.fsync(),
            INode::Symlink(file) => file.fsync(),
            INode::Directory(dir) => dir.fsync(),
        }
    }

    /// Called when the last fd is closed. Flushes dirty file data.
    pub fn close(&self) -> Result<()> {
        match self {
            INode::FileLike(file) => file.close(),
            _ => Ok(()),
        }
    }

    /// `readlink(2)`.
    pub fn readlink(&self) -> Result<Cow<'_, str>> {
        match self {
            INode::FileLike(file) => file.readlink(),
            INode::Symlink(file) => file.linked_to(),
            INode::Directory(dir) => dir.readlink(),
        }
    }

    /// `chmod(2)`
    pub fn chmod(&self, mode: FileMode) -> Result<()> {
        match self {
            INode::FileLike(file) => file.chmod(mode),
            INode::Directory(dir) => dir.chmod(mode),
            INode::Symlink(_) => Ok(()), // symlink chmod is a no-op on Linux too
        }
    }

    /// `utimes(2)` / `utimensat(2)`
    pub fn set_times(&self, atime_secs: Option<isize>, mtime_secs: Option<isize>) -> Result<()> {
        match self {
            INode::FileLike(file) => file.set_times(atime_secs, mtime_secs),
            INode::Directory(dir) => dir.set_times(atime_secs, mtime_secs),
            INode::Symlink(sym) => sym.set_times(atime_secs, mtime_secs),
        }
    }

    /// `chown(2)`
    pub fn chown(&self, uid: UId, gid: GId) -> Result<()> {
        match self {
            INode::FileLike(file) => file.chown(uid, gid),
            INode::Directory(dir) => dir.chown(uid, gid),
            INode::Symlink(_) => Ok(()),
        }
    }
}

impl fmt::Debug for INode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            INode::FileLike(file) => fmt::Debug::fmt(file, f),
            INode::Directory(dir) => fmt::Debug::fmt(dir, f),
            INode::Symlink(symlink) => fmt::Debug::fmt(symlink, f),
        }
    }
}

impl From<Arc<dyn FileLike>> for INode {
    fn from(file: Arc<dyn FileLike>) -> Self {
        INode::FileLike(file)
    }
}

impl From<Arc<dyn Directory>> for INode {
    fn from(dir: Arc<dyn Directory>) -> Self {
        INode::Directory(dir)
    }
}

impl From<Arc<dyn Symlink>> for INode {
    fn from(symlink: Arc<dyn Symlink>) -> Self {
        INode::Symlink(symlink)
    }
}
