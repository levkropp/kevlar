// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (Linux statx(2) man page).
use crate::fs::path::Path;
use crate::prelude::*;
use crate::process::current_process;
use crate::syscalls::{CwdOrFd, StackPathBuf, SyscallHandler};
use kevlar_platform::address::UserVAddr;

const AT_EMPTY_PATH: i32 = 0x1000;
const AT_SYMLINK_NOFOLLOW: i32 = 0x100;
const STATX_BASIC_STATS: u32 = 0x07ff;
const STATX_MNT_ID: u32 = 0x1000;

#[repr(C)]
struct StatxTimestamp {
    tv_sec: i64,
    tv_nsec: u32,
    _pad: i32,
}

#[repr(C)]
struct StatxBuf {
    stx_mask: u32,
    stx_blksize: u32,
    stx_attributes: u64,
    stx_nlink: u32,
    stx_uid: u32,
    stx_gid: u32,
    stx_mode: u16,
    _spare0: u16,
    stx_ino: u64,
    stx_size: u64,
    stx_blocks: u64,
    stx_attributes_mask: u64,
    stx_atime: StatxTimestamp,
    stx_btime: StatxTimestamp,
    stx_ctime: StatxTimestamp,
    stx_mtime: StatxTimestamp,
    stx_rdev_major: u32,
    stx_rdev_minor: u32,
    stx_dev_major: u32,
    stx_dev_minor: u32,
    stx_mnt_id: u64,
    stx_dio_mem_align: u32,
    stx_dio_offset_align: u32,
    _spare3: [u64; 12],
}

impl<'a> SyscallHandler<'a> {
    pub fn sys_statx(
        &mut self,
        dirfd: CwdOrFd,
        pathname: usize,
        flags: i32,
        _mask: u32,
        buf: UserVAddr,
    ) -> Result<isize> {
        let current = current_process();
        let follow_symlink = (flags & AT_SYMLINK_NOFOLLOW) == 0;

        let stat = if (flags & AT_EMPTY_PATH) != 0 && pathname == 0 {
            match &dirfd {
                CwdOrFd::Fd(fd) => current.get_opened_file_by_fd(*fd)?.inode().stat()?,
                CwdOrFd::AtCwd => {
                    let root_fs_arc = current.root_fs();
                    root_fs_arc.lock_no_irq().lookup(Path::new("/"))?.stat()?
                }
            }
        } else {
            let spb = StackPathBuf::from_user(pathname)?;
            let root_fs_arc = current.root_fs();
            let root_fs = root_fs_arc.lock_no_irq();
            let opened_files = current.opened_files_no_irq();
            let path_comp = root_fs.lookup_path_at(
                &opened_files,
                &dirfd,
                spb.as_path(),
                follow_symlink,
            )?;
            path_comp.inode.stat()?
        };

        let stx = StatxBuf {
            stx_mask: STATX_BASIC_STATS | STATX_MNT_ID,
            stx_blksize: 4096,
            stx_attributes: 0,
            stx_nlink: stat.nlink.as_usize() as u32,
            stx_uid: stat.uid.as_u32(),
            stx_gid: stat.gid.as_u32(),
            stx_mode: stat.mode.as_u32() as u16,
            _spare0: 0,
            stx_ino: stat.inode_no.as_u64(),
            stx_size: stat.size.0 as u64,
            stx_blocks: stat.blocks.as_isize() as u64,
            stx_attributes_mask: 0,
            stx_atime: StatxTimestamp { tv_sec: stat.atime.as_isize() as i64, tv_nsec: stat.atime_nsec.as_isize() as u32, _pad: 0 },
            stx_btime: StatxTimestamp { tv_sec: 0, tv_nsec: 0, _pad: 0 },
            stx_ctime: StatxTimestamp { tv_sec: stat.ctime.as_isize() as i64, tv_nsec: stat.ctime_nsec.as_isize() as u32, _pad: 0 },
            stx_mtime: StatxTimestamp { tv_sec: stat.mtime.as_isize() as i64, tv_nsec: stat.mtime_nsec.as_isize() as u32, _pad: 0 },
            stx_rdev_major: 0,
            stx_rdev_minor: 0,
            stx_dev_major: 0,
            stx_dev_minor: 0,
            stx_mnt_id: 0,
            stx_dio_mem_align: 0,
            stx_dio_offset_align: 0,
            _spare3: [0; 12],
        };
        buf.write(&stx)?;
        Ok(0)
    }
}
