// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
// Allow the bad bit mask of O_RDONLY
#![allow(clippy::bad_bit_mask)]

use super::{
    inode::{DirEntry, Directory, FileLike, INode},
    path::PathBuf,
};
use crate::ctypes::c_int;
use crate::fs::inode::PollStatus;
use crate::prelude::*;
use crate::user_buffer::UserBufferMut;
use crate::{net::*, user_buffer::UserBuffer};
use bitflags::bitflags;
use crossbeam::atomic::AtomicCell;

const FD_MAX: c_int = 1024;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct OpenFlags: i32 {
        const O_RDONLY = 0o0;
        const O_WRONLY = 0o1;
        const O_RDWR = 0o2;
        const O_CREAT = 0o100;
        const O_EXCL = 0o200;
        const O_NOCTTY = 0o400; // TODO:
        const O_TRUNC = 0o1000;
        const O_APPEND = 0o2000;
        const O_NONBLOCK = 0o4000;
        const O_DIRECTORY = 0o200000;
        const O_NOFOLLOW = 0o400000;
        const O_DSYNC    = 0o10000;
        const O_SYNC     = 0o4010000;
        const O_CLOEXEC  = 0o2000000;
        /// O_TMPFILE: create an unnamed temporary file. We treat it as O_CREAT
        /// for simplicity — the file gets a name but apk's atomic rename works.
        const O_TMPFILE = 0o20200000;
    }
}

pub use kevlar_vfs::inode::OpenOptions;

impl From<OpenFlags> for OpenOptions {
    fn from(flags: OpenFlags) -> OpenOptions {
        OpenOptions {
            nonblock: flags.contains(OpenFlags::O_NONBLOCK),
            close_on_exec: flags.contains(OpenFlags::O_CLOEXEC),
            append: flags.contains(OpenFlags::O_APPEND),
            access_mode: flags.bits() & 0o3, // O_RDONLY=0, O_WRONLY=1, O_RDWR=2
            sync: flags.contains(OpenFlags::O_SYNC) || flags.contains(OpenFlags::O_DSYNC),
        }
    }
}

/// A file descriptor.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct Fd(c_int);

impl Fd {
    pub const fn new(value: i32) -> Fd {
        Fd(value)
    }

    pub const fn as_int(self) -> c_int {
        self.0
    }

    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }
}

/// Represents a path component.
///
/// This is mainly used for resolving relative paths.
///
/// For example, in `/tmp/foo.txt`, `tmp` and `foo.txt` have separate `PathComponent`
/// instances.
#[derive(Clone)]
pub struct PathComponent {
    /// The parent directory. `None` if this is the root directory.
    pub parent_dir: Option<Arc<PathComponent>>,
    /// THe component name (e.g. `tmp` or `foo.txt` in `/tmp/foo.txt`).
    pub name: String,
    /// The referenced inode.
    pub inode: INode,
}

impl PathComponent {
    /// Creates an anonymous path.
    ///
    /// Sometimes you need to use this to implmenet file-like objects that are
    /// not reachable from the root directory (e.g. unnamed pipes).
    pub fn new_anonymous(inode: INode) -> Arc<PathComponent> {
        Arc::new(PathComponent {
            parent_dir: None,
            name: String::new(), // Empty string avoids heap allocation.
            inode,
        })
    }

    /// Resolves into the absolute path.
    pub fn resolve_absolute_path(&self) -> PathBuf {
        if self.parent_dir.is_some() {
            let mut path = String::from(&self.name);
            let mut parent_dir = &self.parent_dir;
            // Visit its ancestor directories...
            while let Some(path_comp) = parent_dir {
                path = path_comp.name.clone() + "/" + &path;
                parent_dir = &path_comp.parent_dir;
            }

            // The last parent_dir is the root directory and its name is empty. Thus,
            // the computed path must be an absolute path.
            debug_assert!(path.starts_with('/'));
            PathBuf::from(path)
        } else if self.name.starts_with('/') {
            // Flat path — name contains the full absolute path.
            PathBuf::from(self.name.clone())
        } else {
            // Root directory or anonymous.
            PathBuf::from("/")
        }
    }
}

/// An opened file.
///
/// This instance can be shared with multiple processes because of fork(2).
pub struct OpenedFile {
    path: Arc<PathComponent>,
    pos: AtomicCell<usize>,
    options: AtomicCell<OpenOptions>,
    /// Cached from FileLike::is_seekable() at open time. Avoids a vtable
    /// dispatch on every read/write for non-seekable files (pipes, sockets).
    seekable: bool,
}

impl Drop for OpenedFile {
    fn drop(&mut self) {
        // Release any flock locks held by this open file description.
        let ofd = self as *const OpenedFile as usize;
        crate::syscalls::flock::release_all_flocks(ofd);
    }
}

impl OpenedFile {
    pub fn new(path: Arc<PathComponent>, options: OpenOptions, pos: usize) -> OpenedFile {
        let seekable = path.inode.is_seekable();
        OpenedFile {
            path,
            pos: AtomicCell::new(pos),
            options: AtomicCell::new(options),
            seekable,
        }
    }

    pub fn as_file(&self) -> Result<&Arc<dyn FileLike>> {
        self.path.inode.as_file()
    }

    pub fn as_dir(&self) -> Result<&Arc<dyn Directory>> {
        self.path.inode.as_dir()
    }

    pub fn pos(&self) -> usize {
        self.pos.load()
    }

    pub fn set_pos(&self, new_pos: usize) {
        self.pos.store(new_pos);
    }

    pub fn options(&self) -> OpenOptions {
        self.options.load()
    }

    pub fn path(&self) -> &Arc<PathComponent> {
        &self.path
    }

    pub fn inode(&self) -> &INode {
        &self.path.inode
    }

    #[inline]
    pub fn is_seekable(&self) -> bool {
        self.seekable
    }

    #[inline]
    pub fn read(&self, buf: UserBufferMut<'_>) -> Result<usize> {
        let file = self.as_file()?;

        // Fast path for non-seekable files (pipes, /dev/null, sockets):
        // skip pos load/store — they don't use file position.
        if !self.seekable {
            let options = self.options.load();
            return file.read(0, buf, &options);
        }

        let options = self.options.load();
        let pos = self.pos();
        let read_len = file.read(pos, buf, &options)?;
        if read_len > 0 {
            self.pos.fetch_add(read_len);
        }
        Ok(read_len)
    }

    #[inline]
    pub fn write(&self, buf: UserBuffer<'_>) -> Result<usize> {
        let file = self.as_file()?;

        // Fast path for non-seekable files (pipes, /dev/null, sockets):
        // skip pos load/store and O_APPEND check.
        if !self.seekable {
            let options = self.options.load();
            return file.write(0, buf, &options);
        }

        let options = self.options.load();
        let pos = if options.append {
            // O_APPEND: always write at the end of the file.
            let size = file.stat()?.size.0 as usize;
            self.pos.store(size);
            size
        } else {
            self.pos()
        };

        let written_len = file.write(pos, buf, &options)?;
        self.pos.fetch_add(written_len);

        // O_SYNC / O_DSYNC: flush data to disk after each write.
        if options.sync {
            file.fsync()?;
        }

        Ok(written_len)
    }

    pub fn set_cloexec(&self, cloexec: bool) {
        // FIXME: Modify LocalOpenedFile as well!
        let mut opts = self.options.load();
        opts.close_on_exec = cloexec;
        self.options.store(opts);
    }

    pub fn set_flags(&self, flags: OpenFlags) -> Result<()> {
        if flags.contains(OpenFlags::O_NONBLOCK) {
            let mut opts = self.options.load();
            opts.nonblock = true;
            self.options.store(opts);
        }

        Ok(())
    }

    pub fn set_nonblock(&self, nonblock: bool) {
        let mut opts = self.options.load();
        opts.nonblock = nonblock;
        self.options.store(opts);
    }

    pub fn fsync(&self) -> Result<()> {
        self.path.inode.fsync()
    }

    pub fn ioctl(&self, cmd: usize, arg: usize) -> Result<isize> {
        self.as_file()?.ioctl(cmd, arg)
    }

    pub fn listen(&self, backlog: i32) -> Result<()> {
        self.as_file()?.listen(backlog)
    }

    pub fn accept(&self) -> Result<(Arc<dyn FileLike>, SockAddr)> {
        // Avoid holding self.options lock by copying.
        let options = self.options();

        self.as_file()?.accept(&options)
    }

    pub fn bind(&self, sockaddr: SockAddr) -> Result<()> {
        self.as_file()?.bind(sockaddr)
    }

    pub fn shutdown(&self, how: ShutdownHow) -> Result<()> {
        self.as_file()?.shutdown(how)
    }

    pub fn getsockname(&self) -> Result<SockAddr> {
        self.as_file()?.getsockname()
    }

    pub fn getpeername(&self) -> Result<SockAddr> {
        self.as_file()?.getpeername()
    }

    pub fn connect(&self, sockaddr: SockAddr) -> Result<()> {
        // Avoid holding self.options lock by copying.
        let options = self.options();

        self.as_file()?.connect(sockaddr, &options)
    }

    pub fn sendto(&self, buf: UserBuffer<'_>, sockaddr: Option<SockAddr>) -> Result<usize> {
        // Avoid holding self.options lock by copying.
        let options = self.options();

        self.as_file()?.sendto(buf, sockaddr, &options)
    }

    pub fn recvfrom(
        &self,
        buf: UserBufferMut<'_>,
        flags: RecvFromFlags,
    ) -> Result<(usize, SockAddr)> {
        // Avoid holding self.options lock by copying.
        let options = self.options();

        self.as_file()?.recvfrom(buf, flags, &options)
    }

    pub fn poll(&self) -> Result<PollStatus> {
        self.as_file()?.poll()
    }

    pub fn readdir(&self) -> Result<Option<DirEntry>> {
        // Avoid holding self.pos lock by copying.
        let pos = self.pos();

        let entry = self.as_dir()?.readdir(pos)?;
        self.pos.fetch_add(1);
        Ok(entry)
    }
}

/// A opened file with process-local fields.
#[derive(Clone)]
struct LocalOpenedFile {
    opened_file: Arc<OpenedFile>,
    close_on_exec: bool,
}

/// The opened file table.
#[derive(Clone)]
pub struct OpenedFileTable {
    files: Vec<Option<LocalOpenedFile>>,
}

impl OpenedFileTable {
    pub fn new() -> OpenedFileTable {
        OpenedFileTable {
            files: Vec::new(),
        }
    }

    /// Returns the number of open file descriptors.
    pub fn count_open(&self) -> usize {
        self.files.iter().filter(|f| f.is_some()).count()
    }

    /// Returns the capacity (highest possible fd + 1).
    pub fn table_size(&self) -> usize {
        self.files.len()
    }

    /// Iterates over open file descriptors, yielding (fd_number, &OpenedFile).
    pub fn iter_open(&self) -> impl Iterator<Item = (usize, &Arc<OpenedFile>)> {
        self.files.iter().enumerate().filter_map(|(i, slot)| {
            slot.as_ref().map(|local| (i, &local.opened_file))
        })
    }

    /// Resolves the opened file by the file descriptor.
    pub fn get(&self, fd: Fd) -> Result<&Arc<OpenedFile>> {
        match self.files.get(fd.as_usize()) {
            Some(Some(LocalOpenedFile { opened_file, .. })) => Ok(opened_file),
            _ => Err(Error::new(Errno::EBADF)),
        }
    }

    /// Returns the per-fd close-on-exec flag (FD_CLOEXEC).
    pub fn get_cloexec(&self, fd: Fd) -> Result<bool> {
        match self.files.get(fd.as_usize()) {
            Some(Some(local)) => Ok(local.close_on_exec),
            _ => Err(Error::new(Errno::EBADF)),
        }
    }

    /// Sets the per-fd close-on-exec flag (FD_CLOEXEC).
    pub fn set_cloexec(&mut self, fd: Fd, cloexec: bool) -> Result<()> {
        match self.files.get_mut(fd.as_usize()) {
            Some(Some(local)) => {
                local.close_on_exec = cloexec;
                Ok(())
            }
            _ => Err(Error::new(Errno::EBADF)),
        }
    }

    /// Closes an opened file. Calls FileLike::close() to flush dirty data.
    pub fn close(&mut self, fd: Fd) -> Result<()> {
        match self.files.get_mut(fd.as_usize()) {
            Some(slot @ Some(_)) => {
                // Call close() on the file before dropping it.
                if let Some(local) = slot.as_ref() {
                    let _ = local.opened_file.path.inode.close();
                }
                *slot = None;
            }
            _ => return Err(Errno::EBADF.into()),
        }

        Ok(())
    }

    /// Opens a file.
    pub fn open(&mut self, path: Arc<PathComponent>, options: OpenOptions) -> Result<Fd> {
        self.alloc_fd(None).and_then(|fd| {
            let seekable = path.inode.is_seekable();
            self.open_with_fixed_fd(
                fd,
                Arc::new(OpenedFile {
                    path,
                    options: AtomicCell::new(options),
                    pos: AtomicCell::new(0),
                    seekable,
                }),
                options,
            )
            .map(|_| fd)
        })
    }

    /// Opens a file with the given file descriptor.
    ///
    /// Returns `EBADF` if the file descriptor is already in use.
    pub fn open_with_fixed_fd(
        &mut self,
        fd: Fd,
        mut opened_file: Arc<OpenedFile>,
        options: OpenOptions,
    ) -> Result<()> {
        if let INode::FileLike(file) = &opened_file.path.inode {
            if let Some(new_inode) = file.open(&options)? {
                // Replace inode if FileLike::open returned Some. Currently it's
                // used only for /dev/ptmx.
                let new_path = Arc::new(PathComponent {
                    name: opened_file.path.name.clone(),
                    parent_dir: opened_file.path.parent_dir.clone(),
                    inode: new_inode.into(),
                });
                let seekable = new_path.inode.is_seekable();
                opened_file = Arc::new(OpenedFile {
                    pos: AtomicCell::new(0),
                    options: AtomicCell::new(options),
                    path: new_path,
                    seekable,
                })
            }
        }

        match self.files.get_mut(fd.as_usize()) {
            Some(Some(_)) => {
                return Err(Error::with_message(
                    Errno::EBADF,
                    "already opened at the fd",
                ));
            }
            Some(entry @ None) => {
                *entry = Some(LocalOpenedFile {
                    opened_file,
                    close_on_exec: options.close_on_exec,
                });
            }
            None if fd.as_int() >= FD_MAX => {
                return Err(Errno::EBADF.into());
            }
            None => {
                self.files.resize(fd.as_usize() + 1, None);
                self.files[fd.as_usize()] = Some(LocalOpenedFile {
                    opened_file,
                    close_on_exec: options.close_on_exec,
                });
            }
        }

        Ok(())
    }

    /// Duplicates a file descriptor.
    ///
    /// If `gte` is `Some`, a new file descriptor will be greater than or equals
    /// to that value.
    pub fn dup(&mut self, fd: Fd, gte: Option<i32>, options: OpenOptions) -> Result<Fd> {
        let opened_file = match self.files.get(fd.as_usize()) {
            Some(Some(opened_file)) => opened_file.opened_file.clone(),
            _ => return Err(Errno::EBADF.into()),
        };

        self.alloc_fd(gte).and_then(|fd| {
            self.open_with_fixed_fd(fd, opened_file, options)
                .map(|_| fd)
        })
    }

    /// Duplicates a file descriptor into the given file descriptor `new`.
    pub fn dup2(&mut self, old: Fd, new: Fd, options: OpenOptions) -> Result<()> {
        let opened_file = match self.files.get(old.as_usize()) {
            Some(Some(opened_file)) => opened_file.opened_file.clone(),
            _ => return Err(Errno::EBADF.into()),
        };

        if let Some(Some(_)) = self.files.get(new.as_usize()) {
            self.close(new).ok();
        }

        self.open_with_fixed_fd(new, opened_file, options)?;
        Ok(())
    }

    /// Closes all opened files.
    pub fn close_all(&mut self) {
        self.files.clear();
    }

    /// Closes opened files with `CLOEXEC` set.
    pub fn close_cloexec_files(&mut self) {
        for slot in &mut self.files {
            if matches!(
                slot,
                Some(LocalOpenedFile {
                    close_on_exec: true,
                    ..
                })
            ) {
                *slot = None;
            }
        }
    }

    /// Allocates an unused fd. Note that this method does not any reservations
    /// for the fd: the caller must register it before unlocking this table.
    fn alloc_fd(&mut self, gte: Option<i32>) -> Result<Fd> {
        // Use per-process RLIMIT_NOFILE if available, else fall back to FD_MAX.
        let limit = crate::process::current_process_option()
            .map(|p| {
                let rl = p.rlimits();
                rl[7][0] as i32 // RLIMIT_NOFILE soft limit
            })
            .unwrap_or(FD_MAX);
        let limit = limit.min(FD_MAX); // Never exceed the hard table max

        // POSIX: open() must return the lowest available fd number.
        let start = gte.unwrap_or(0);
        for i in start..limit {
            if matches!(self.files.get(i as usize), Some(None) | None) {
                return Ok(Fd::new(i));
            }
        }
        Err(Error::new(Errno::EMFILE))
    }
}

impl Default for OpenedFileTable {
    fn default() -> OpenedFileTable {
        OpenedFileTable::new()
    }
}
