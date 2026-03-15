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
mod tty;
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
        SERIAL_TTY.init(|| Arc::new(Tty::new("serial")));
        PTMX.init(|| Arc::new(Ptmx::new(pts_dir)));

        root_dir.add_file("null", NULL_FILE.clone());
        root_dir.add_file("zero", Arc::new(ZeroFile::new()) as Arc<dyn FileLike>);
        root_dir.add_file("tty", SERIAL_TTY.clone() as Arc<dyn FileLike>);
        root_dir.add_file("console", SERIAL_TTY.clone() as Arc<dyn FileLike>);
        root_dir.add_file("ttyS0", SERIAL_TTY.clone() as Arc<dyn FileLike>);
        root_dir.add_file("ptmx", PTMX.clone() as Arc<dyn FileLike>);
        root_dir.add_file("kmsg", Arc::new(KmsgFile) as Arc<dyn FileLike>);
        root_dir.add_file("urandom", Arc::new(UrandomFile) as Arc<dyn FileLike>);
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
