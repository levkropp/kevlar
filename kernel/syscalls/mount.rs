// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! mount(2) and umount2(2) syscall handlers.
//!
//! Provenance: Own (Linux mount(2), umount2(2) man pages).
use crate::{
    cgroups::cgroupfs::CgroupFs,
    ctypes::c_int,
    fs::{
        mount::MountTable,
        procfs::PROC_FS,
        sysfs::SYS_FS,
        tmpfs::TmpFs,
    },
    prelude::*,
    process::current_process,
    syscalls::{resolve_path, SyscallHandler},
    user_buffer::UserCStr,
};
use kevlar_platform::address::UserVAddr;
use kevlar_vfs::file_system::FileSystem;

impl<'a> SyscallHandler<'a> {
    pub fn sys_mount(
        &mut self,
        _source: UserVAddr,
        target_ptr: UserVAddr,
        fstype_ptr: UserVAddr,
        _flags: c_int,
        _data: usize,
    ) -> Result<isize> {
        const PATH_MAX: usize = 256;
        let target_path = resolve_path(target_ptr.value())?;
        let fstype_str = UserCStr::new(fstype_ptr, PATH_MAX)?;
        let fstype = fstype_str.as_str();

        // Create the appropriate filesystem.
        let fs: Arc<dyn FileSystem> = match fstype {
            "proc" => PROC_FS.clone(),
            "sysfs" => SYS_FS.clone(),
            "tmpfs" => Arc::new(TmpFs::new()),
            "ext2" => {
                // Mount ext2 from the global block device.
                kevlar_ext2::mount_ext2()?
            }
            "devtmpfs" | "devpts" => {
                // Our devfs is always mounted; silently succeed.
                return Ok(0);
            }
            "cgroup2" | "cgroup" => {
                CgroupFs::new_or_get()
            }
            _ => {
                debug_warn!("mount: unsupported filesystem type: {}", fstype);
                return Err(Errno::ENODEV.into());
            }
        };

        // Look up the target directory and mount.
        let root_fs = current_process().root_fs();
        let mut root_fs = root_fs.lock();

        // Ensure target directory exists. If not, try to create it.
        let dir = match root_fs.lookup_dir(&target_path) {
            Ok(d) => d,
            Err(_) => {
                // Try to create the directory (e.g., /run, /dev/shm).
                // This mimics `mkdir -p` for the mount target.
                if let Some((parent, name)) = target_path.parent_and_basename() {
                    let parent_dir = root_fs.lookup_dir(parent)?;
                    let inode = parent_dir.create_dir(name, kevlar_vfs::stat::FileMode::new(0o755))?;
                    match inode {
                        kevlar_vfs::inode::INode::Directory(d) => d,
                        _ => return Err(Errno::ENOTDIR.into()),
                    }
                } else {
                    return Err(Errno::ENOENT.into());
                }
            }
        };

        root_fs.mount(dir, fs.clone())?;

        // Record in mount table for /proc/mounts.
        MountTable::add(fstype, target_path.as_str());

        Ok(0)
    }

    pub fn sys_umount2(
        &mut self,
        target_ptr: UserVAddr,
        _flags: c_int,
    ) -> Result<isize> {
        let target_path = resolve_path(target_ptr.value())?;

        // Remove from mount table.
        MountTable::remove(target_path.as_str());

        // We don't actually unmount from the VFS — the mount point stays.
        // This is fine for systemd which rarely unmounts at runtime.
        Ok(0)
    }
}
