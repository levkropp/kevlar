// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use core::fmt;

use crate::{
    fs::{
        inode::{FileLike, INodeNo},
        opened_file::OpenOptions,
        stat::{FileMode, Stat, S_IFCHR},
    },
    result::Result,
    user_buffer::{UserBuffer, UserBufWriter, UserBufferMut},
};

/// The `/dev/zero` file — reads as infinite zeros, writes are absorbed.
pub(super) struct ZeroFile {}

impl ZeroFile {
    pub fn new() -> ZeroFile {
        ZeroFile {}
    }
}

impl fmt::Debug for ZeroFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DevZero").finish()
    }
}

impl FileLike for ZeroFile {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            inode_no: INodeNo::new(3),
            mode: FileMode::new(S_IFCHR | 0o666),
            rdev: kevlar_vfs::stat::DevId::new((1 << 8) | 5), // major=1 minor=5
            ..Stat::zeroed()
        })
    }

    fn read(
        &self,
        _offset: usize,
        buf: UserBufferMut<'_>,
        _options: &OpenOptions,
    ) -> Result<usize> {
        // Fill the buffer with zeros directly (single usercopy, not 256-byte chunks).
        let len = buf.len();
        let mut writer = UserBufWriter::from(buf);
        writer.fill(0, len)?;
        Ok(len)
    }

    fn write(&self, _offset: usize, buf: UserBuffer<'_>, _options: &OpenOptions) -> Result<usize> {
        Ok(buf.len())
    }
}
