// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use self::{null::NullFile, zero::ZeroFile, tty::Tty};

use crate::{
    fs::{
        file_system::FileSystem,
        inode::{Directory, FileLike},
    },
    result::Result,
    tty::pty::Ptmx,
};
use alloc::sync::Arc;
use kevlar_utils::once::Once;

use super::tmpfs::TmpFs;

mod null;
pub mod tty;
mod zero;

pub static DEV_FS: Once<Arc<DevFs>> = Once::new();
static NULL_FILE: Once<Arc<dyn FileLike>> = Once::new();
pub static SERIAL_TTY: Once<Arc<Tty>> = Once::new();
pub static PTMX: Once<Arc<Ptmx>> = Once::new();

pub struct DevFs(TmpFs);

impl DevFs {
    pub fn new() -> DevFs {
        let tmpfs = TmpFs::new();
        let root_dir = tmpfs.root_tmpfs_dir();
        let pts_dir = root_dir.add_dir("pts");

        NULL_FILE.init(|| Arc::new(NullFile::new()) as Arc<dyn FileLike>);
        // makedev(major, minor) = (minor & 0xff) | (major << 8)
        // /dev/console: major=5, minor=1 → 0x501
        // /dev/tty:     major=5, minor=0 → 0x500
        // /dev/ttyS0:   major=4, minor=64 → 0x440
        // All share the same physical serial device.
        // ttyS0: major=4, minor=64 → rdev=0x440
        SERIAL_TTY.init(|| Arc::new(Tty::with_rdev("ttyS0", 0x440)));
        PTMX.init(|| Arc::new(Ptmx::new(pts_dir)));

        root_dir.add_file("null", NULL_FILE.clone());
        root_dir.add_file("zero", Arc::new(ZeroFile::new()) as Arc<dyn FileLike>);
        root_dir.add_file("tty", SERIAL_TTY.clone() as Arc<dyn FileLike>);
        root_dir.add_file("console", SERIAL_TTY.clone() as Arc<dyn FileLike>);
        root_dir.add_file("ttyS0", SERIAL_TTY.clone() as Arc<dyn FileLike>);
        root_dir.add_file("ptmx", PTMX.clone() as Arc<dyn FileLike>);
        root_dir.add_file("kmsg", Arc::new(KmsgFile) as Arc<dyn FileLike>);
        root_dir.add_file("urandom", Arc::new(UrandomFile) as Arc<dyn FileLike>);
        root_dir.add_file("random", Arc::new(UrandomFile) as Arc<dyn FileLike>);
        root_dir.add_file("full", Arc::new(FullFile) as Arc<dyn FileLike>);
        // /dev/shm directory for POSIX shared memory.
        root_dir.add_dir("shm");

        DevFs(tmpfs)
    }
}

impl FileSystem for DevFs {
    fn root_dir(&self) -> Result<Arc<dyn Directory>> {
        self.0.root_dir()
    }
}

pub fn init() {
    DEV_FS.init(|| Arc::new(DevFs::new()));
}

/// Look up a device by Linux major:minor numbers.
/// Returns the real device FileLike object for supported devices.
pub fn lookup_device(major: u32, minor: u32) -> Option<Arc<dyn FileLike>> {
    match (major, minor) {
        (1, 3) => Some(NULL_FILE.clone()),
        (1, 5) => Some(Arc::new(ZeroFile::new()) as Arc<dyn FileLike>),
        (1, 7) => Some(Arc::new(FullFile) as Arc<dyn FileLike>),
        (1, 8) | (1, 9) => Some(Arc::new(UrandomFile) as Arc<dyn FileLike>),
        (1, 11) => Some(Arc::new(KmsgFile) as Arc<dyn FileLike>),
        (4, 64) | (5, 0) | (5, 1) => Some(SERIAL_TTY.clone() as Arc<dyn FileLike>),
        (5, 2) => Some(PTMX.clone() as Arc<dyn FileLike>),
        _ => None,
    }
}

/// A device node created by mknod(2). Stores mode + rdev, and its
/// `open()` method redirects to the real device via `lookup_device`.
pub struct DeviceNodeFile {
    mode: u32,
    rdev: u32,
}

impl DeviceNodeFile {
    pub fn new(mode: u32, rdev: u32) -> Self {
        Self { mode, rdev }
    }

    fn major(&self) -> u32 {
        (self.rdev >> 8) & 0xfff
    }

    fn minor(&self) -> u32 {
        (self.rdev & 0xff) | ((self.rdev >> 12) & 0xfff00)
    }
}

impl fmt::Debug for DeviceNodeFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceNodeFile({}:{})", self.major(), self.minor())
    }
}

impl FileLike for DeviceNodeFile {
    fn stat(&self) -> kevlar_vfs::result::Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(self.mode),
            rdev: kevlar_vfs::stat::DevId::new(self.rdev as usize),
            ..Stat::zeroed()
        })
    }

    fn open(&self, _options: &OpenOptions) -> kevlar_vfs::result::Result<Option<Arc<dyn FileLike>>> {
        // Redirect to the real device based on major:minor.
        match lookup_device(self.major(), self.minor()) {
            Some(dev) => Ok(Some(dev)),
            None => Ok(None), // no matching device — use this node as-is
        }
    }

    fn read(&self, _offset: usize, _buf: UserBufferMut<'_>, _options: &OpenOptions) -> kevlar_vfs::result::Result<usize> {
        // Reads go through the real device after open() redirects.
        Err(Error::new(Errno::ENXIO))
    }

    fn write(&self, _offset: usize, _buf: UserBuffer<'_>, _options: &OpenOptions) -> kevlar_vfs::result::Result<usize> {
        Err(Error::new(Errno::ENXIO))
    }
}

// ── /dev/kmsg: kernel log (write = serial output, read = empty) ─────

use core::fmt;
use kevlar_vfs::{
    inode::OpenOptions,
    result::{Errno, Error},
    stat::{FileMode, Stat, S_IFCHR},
    user_buffer::{UserBufReader, UserBuffer, UserBufferMut},
};

struct KmsgFile;

impl fmt::Debug for KmsgFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "KmsgFile")
    }
}

impl FileLike for KmsgFile {
    fn stat(&self) -> kevlar_vfs::result::Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFCHR | 0o644),
            ..Stat::zeroed()
        })
    }

    fn read(&self, _offset: usize, _buf: UserBufferMut<'_>, _options: &OpenOptions) -> kevlar_vfs::result::Result<usize> {
        Ok(0) // empty
    }

    fn write(&self, _offset: usize, buf: UserBuffer<'_>, _options: &OpenOptions) -> kevlar_vfs::result::Result<usize> {
        // Write to serial (kernel log).
        let mut data = [0u8; 512];
        let mut reader = UserBufReader::from(buf);
        let n = reader.read_bytes(&mut data)?;
        if n > 0 {
            if let Ok(s) = core::str::from_utf8(&data[..n]) {
                info!("kmsg: {}", s.trim_end());
            }
        }
        Ok(n)
    }
}

// ── /dev/urandom: random bytes ──────────────────────────────────────

struct UrandomFile;

impl fmt::Debug for UrandomFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "UrandomFile")
    }
}

impl FileLike for UrandomFile {
    fn stat(&self) -> kevlar_vfs::result::Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFCHR | 0o666),
            rdev: kevlar_vfs::stat::DevId::new((1 << 8) | 9), // major=1 minor=9
            ..Stat::zeroed()
        })
    }

    fn read(&self, _offset: usize, buf: UserBufferMut<'_>, _options: &OpenOptions) -> kevlar_vfs::result::Result<usize> {
        use kevlar_vfs::user_buffer::UserBufWriter;
        let len = buf.len();
        let mut writer = UserBufWriter::from(buf);
        // Fill with random bytes.
        let mut tmp = [0u8; 64];
        let mut written = 0;
        while written < len {
            kevlar_platform::random::rdrand_fill(&mut tmp);
            let chunk = core::cmp::min(tmp.len(), len - written);
            writer.write_bytes(&tmp[..chunk])?;
            written += chunk;
        }
        Ok(written)
    }
}

// ── /dev/full: always ENOSPC on write ───────────────────────────────

struct FullFile;

impl fmt::Debug for FullFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FullFile")
    }
}

impl FileLike for FullFile {
    fn stat(&self) -> kevlar_vfs::result::Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFCHR | 0o666),
            ..Stat::zeroed()
        })
    }

    fn read(&self, _offset: usize, buf: UserBufferMut<'_>, _options: &OpenOptions) -> kevlar_vfs::result::Result<usize> {
        // Read returns zeros (like /dev/zero).
        use kevlar_vfs::user_buffer::UserBufWriter;
        let len = buf.len();
        let mut writer = UserBufWriter::from(buf);
        let zeros = [0u8; 64];
        let mut written = 0;
        while written < len {
            let chunk = core::cmp::min(zeros.len(), len - written);
            writer.write_bytes(&zeros[..chunk])?;
            written += chunk;
        }
        Ok(written)
    }

    fn write(&self, _offset: usize, _buf: UserBuffer<'_>, _options: &OpenOptions) -> kevlar_vfs::result::Result<usize> {
        Err(Error::new(Errno::ENOSPC))
    }
}
