// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use core::fmt;

use crate::{
    fs::{
        inode::{FileLike, INodeNo, PollStatus},
        opened_file::OpenOptions,
        stat::{FileMode, Stat, S_IFCHR},
    },
    result::Result,
    user_buffer::UserBuffer,
    user_buffer::UserBufferMut,
};

/// The `/dev/null` file.
pub(super) struct NullFile {}

impl NullFile {
    pub fn new() -> NullFile {
        NullFile {}
    }
}

impl fmt::Debug for NullFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DevNull").finish()
    }
}

impl FileLike for NullFile {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            inode_no: INodeNo::new(2),
            mode: FileMode::new(S_IFCHR | 0o666),
            rdev: kevlar_vfs::stat::DevId::new((1 << 8) | 3), // major=1 minor=3
            ..Stat::zeroed()
        })
    }

    fn read(
        &self,
        _offset: usize,
        _buf: UserBufferMut<'_>,
        _options: &OpenOptions,
    ) -> Result<usize> {
        Ok(0)
    }

    fn write(&self, _offset: usize, buf: UserBuffer<'_>, _options: &OpenOptions) -> Result<usize> {
        Ok(buf.len())
    }

    fn poll(&self) -> Result<PollStatus> {
        // /dev/null is always ready for writing and reading (returns EOF).
        Ok(PollStatus::POLLOUT | PollStatus::POLLIN)
    }
}
