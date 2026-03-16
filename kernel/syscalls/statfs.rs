// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (Linux statfs(2) man page).
use crate::fs::mount::MountTable;
use crate::fs::opened_file::Fd;
use crate::fs::path::Path;
use crate::prelude::*;
use crate::process::current_process;
use crate::syscalls::SyscallHandler;
use kevlar_platform::address::UserVAddr;

const TMPFS_MAGIC: i64 = 0x01021994;
const PROC_SUPER_MAGIC: i64 = 0x9FA0;
const EXT2_SUPER_MAGIC: i64 = 0xEF53;
const SYSFS_MAGIC: i64 = 0x62656572;
const CGROUP2_SUPER_MAGIC: i64 = 0x63677270;

/// Linux struct statfs (x86_64 / arm64).
#[repr(C)]
struct StatfsBuf {
    f_type: i64,
    f_bsize: i64,
    f_blocks: i64,
    f_bfree: i64,
    f_bavail: i64,
    f_files: i64,
    f_ffree: i64,
    f_fsid: [i32; 2],
    f_namelen: i64,
    f_frsize: i64,
    f_flags: i64,
    f_spare: [i64; 4],
}

impl StatfsBuf {
    fn tmpfs() -> StatfsBuf {
        StatfsBuf {
            f_type: TMPFS_MAGIC,
            f_bsize: 4096,
            f_blocks: 65536,
            f_bfree: 65536,
            f_bavail: 65536,
            f_files: 65536,
            f_ffree: 65536,
            f_fsid: [0, 0],
            f_namelen: 255,
            f_frsize: 4096,
            f_flags: 0,
            f_spare: [0; 4],
        }
    }

    fn procfs() -> StatfsBuf {
        StatfsBuf {
            f_type: PROC_SUPER_MAGIC,
            f_bsize: 4096,
            f_blocks: 0,
            f_bfree: 0,
            f_bavail: 0,
            f_files: 0,
            f_ffree: 0,
            f_fsid: [0, 0],
            f_namelen: 255,
            f_frsize: 4096,
            f_flags: 0,
            f_spare: [0; 4],
        }
    }

    fn ext2() -> StatfsBuf {
        StatfsBuf {
            f_type: EXT2_SUPER_MAGIC,
            f_bsize: 4096,
            f_blocks: 16384,
            f_bfree: 8192,
            f_bavail: 8192,
            f_files: 16384,
            f_ffree: 8192,
            f_fsid: [0, 0],
            f_namelen: 255,
            f_frsize: 4096,
            f_flags: 0,
            f_spare: [0; 4],
        }
    }

    fn sysfs() -> StatfsBuf {
        StatfsBuf { f_type: SYSFS_MAGIC, ..StatfsBuf::procfs() }
    }

    fn cgroup2() -> StatfsBuf {
        StatfsBuf { f_type: CGROUP2_SUPER_MAGIC, ..StatfsBuf::tmpfs() }
    }

    fn for_path(path: &Path) -> StatfsBuf {
        match MountTable::fstype_for_path(path.as_str()).as_deref() {
            Some("proc") => StatfsBuf::procfs(),
            Some("sysfs") => StatfsBuf::sysfs(),
            Some("cgroup2") | Some("cgroup") => StatfsBuf::cgroup2(),
            Some("ext2") | Some("ext3") | Some("ext4") => StatfsBuf::ext2(),
            _ => StatfsBuf::tmpfs(),
        }
    }
}

impl<'a> SyscallHandler<'a> {
    pub fn sys_statfs(&mut self, path: &Path, buf: UserVAddr) -> Result<isize> {
        let sfs = StatfsBuf::for_path(path);
        buf.write(&sfs)?;
        Ok(0)
    }

    pub fn sys_fstatfs(&mut self, fd: Fd, buf: UserVAddr) -> Result<isize> {
        let opened_file = current_process().get_opened_file_by_fd(fd)?;
        // Determine filesystem type from the file's path.
        let path = opened_file.path().resolve_absolute_path();
        let sfs = StatfsBuf::for_path(&path);
        buf.write(&sfs)?;
        Ok(0)
    }
}
