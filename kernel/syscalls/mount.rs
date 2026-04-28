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
        source_opt: Option<UserVAddr>,
        target_ptr: UserVAddr,
        fstype_opt: Option<UserVAddr>,
        flags: c_int,
        _data: usize,
    ) -> Result<isize> {
        const PATH_MAX: usize = 256;
        const MS_RDONLY: c_int = 1;
        #[allow(dead_code)]
        const MS_NOSUID: c_int = 2;
        #[allow(dead_code)]
        const MS_NODEV: c_int = 4;
        #[allow(dead_code)]
        const MS_NOEXEC: c_int = 8;
        const MS_REMOUNT: c_int = 0x20;
        const MS_BIND: c_int = 0x1000;
        const MS_REC: c_int = 0x4000;
        const MS_PRIVATE: c_int = 1 << 18;

        // Flag-only mount operations (no filesystem type needed).
        let fstype_is_null = match fstype_opt {
            Some(ptr) => ptr.value() == 0,
            None => true,
        };
        let has_flag_op = flags & (MS_PRIVATE | MS_REC | MS_REMOUNT | MS_BIND) != 0;
        if has_flag_op && (fstype_is_null || flags & MS_REMOUNT != 0 || flags & MS_BIND != 0) {
            if flags & MS_BIND != 0 {
                // Bind mount: make source visible at target.
                // For file bind mounts (e.g. /dev/console), accept silently.
                if let Some(source_ptr) = source_opt {
                    if source_ptr.value() != 0 {
                        let source_path = resolve_path(source_ptr.value())?;
                        let target_path = resolve_path(target_ptr.value())?;
                        let root_fs = current_process().root_fs();
                        let mut root_fs = root_fs.lock();
                        if let Ok(source_dir) = root_fs.lookup_dir(&source_path) {
                            if let Ok(target_dir) = root_fs.lookup_dir(&target_path) {
                                let bind_fs = alloc::sync::Arc::new(BindFs(source_dir));
                                let _ = root_fs.mount(target_dir, bind_fs);
                                MountTable::add("none", target_path.as_str());
                            }
                        }
                    }
                }
                return Ok(0);
            }
            // MS_PRIVATE, MS_REC, MS_REMOUNT: accept silently.
            return Ok(0);
        }

        let target_path = resolve_path(target_ptr.value())?;
        let fstype_ptr = fstype_opt.ok_or_else(|| crate::result::Error::new(Errno::EINVAL))?;
        let fstype_str = UserCStr::new(fstype_ptr, PATH_MAX)?;
        let fstype = fstype_str.as_str();

        // Create the appropriate filesystem.
        let fs: Arc<dyn FileSystem> = match fstype {
            "proc" => PROC_FS.clone(),
            "sysfs" => SYS_FS.clone(),
            "tmpfs" => Arc::new(TmpFs::new()),
            // K33 Phase 3: dispatch to the kABI fs registry first
            // for any filesystem that loaded Linux's `.ko` module
            // and called `register_filesystem(...)` from init.  As
            // of K33, that's `erofs` (via `make` boot with
            // `kabi-load-erofs=1` cmdline).  ext4 follows once the
            // .ko build path is set up (task #99).  Falls through
            // to the homegrown handlers below if the kABI registry
            // doesn't have the fstype.
            "erofs" => {
                match crate::kabi::fs_adapter::kabi_mount_filesystem(
                    fstype, None, flags as u32, core::ptr::null(),
                ) {
                    Ok(fs) => fs,
                    Err(e) => {
                        debug_warn!(
                            "mount: kABI route for {} failed: {:?}",
                            fstype, e,
                        );
                        return Err(e);
                    }
                }
            }
            "ext2" | "ext3" | "ext4" => {
                // Mount ext2/ext3/ext4 from the global block device.
                // TODO: when ext4.ko is in the kABI registry, route
                // ext4 here through `kabi_mount_filesystem` (task #99).
                kevlar_ext2::mount_ext2()?
            }
            "devtmpfs" => {
                // Mount our real DEV_FS so mknod'd nodes appear.
                crate::fs::devfs::DEV_FS.clone()
            }
            "devpts" => {
                // Our devfs already provides /dev/pts; silently succeed.
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
                    let inode = parent_dir.create_dir(name, kevlar_vfs::stat::FileMode::new(0o755), kevlar_vfs::stat::UId::new(0), kevlar_vfs::stat::GId::new(0))?;
                    match inode {
                        kevlar_vfs::inode::INode::Directory(d) => d,
                        _ => return Err(Errno::ENOTDIR.into()),
                    }
                } else {
                    return Err(Errno::ENOENT.into());
                }
            }
        };

        let is_rdonly = flags & MS_RDONLY != 0;
        if is_rdonly {
            root_fs.mount_readonly(dir, fs.clone())?;
        } else {
            root_fs.mount(dir, fs.clone())?;
        }

        // Record in mount table for /proc/mounts.
        MountTable::add_with_flags(fstype, target_path.as_str(), is_rdonly);

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

/// Minimal bind-mount filesystem wrapper: wraps a directory as a filesystem root.
struct BindFs(Arc<dyn kevlar_vfs::inode::Directory>);

impl kevlar_vfs::file_system::FileSystem for BindFs {
    fn root_dir(&self) -> kevlar_vfs::result::Result<Arc<dyn kevlar_vfs::inode::Directory>> {
        Ok(self.0.clone())
    }
}
