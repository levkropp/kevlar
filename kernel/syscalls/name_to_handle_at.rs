// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
// Minimal name_to_handle_at implementation for mount point detection.
// systemd uses this to determine if /sys, /proc, /dev are mount points.
use crate::fs::mount::MountTable;
use crate::prelude::*;
use crate::process::current_process;
use crate::syscalls::{CwdOrFd, SyscallHandler, StackPathBuf};
use kevlar_platform::address::UserVAddr;

const AT_EMPTY_PATH: i32 = 0x1000;
const MAX_HANDLE_SZ: i32 = 128;

/// Minimal file_handle structure for mount point detection.
/// The kernel fills in handle_bytes and f_handle; we only need mount_id.
#[repr(C)]
struct FileHandle {
    handle_bytes: u32,
    handle_type: i32,
    // f_handle bytes follow, but we use a fixed small handle
}

impl<'a> SyscallHandler<'a> {
    pub fn sys_name_to_handle_at(
        &mut self,
        dirfd: CwdOrFd,
        name_ptr: usize,
        handle_ptr: UserVAddr,
        mount_id_ptr: UserVAddr,
        flags: i32,
    ) -> Result<isize> {
        // Read the user's file_handle to check handle_bytes capacity.
        let user_handle: FileHandle = handle_ptr.read()?;
        if (user_handle.handle_bytes as i32) < 0 || user_handle.handle_bytes > MAX_HANDLE_SZ as u32 {
            return Err(Errno::EINVAL.into());
        }

        // Resolve the path to determine the mount point.
        let full_path = if (flags & AT_EMPTY_PATH) != 0 && name_ptr == 0 {
            // AT_EMPTY_PATH with NULL name: use the fd itself.
            match &dirfd {
                CwdOrFd::AtCwd => alloc::string::String::from("/"),
                CwdOrFd::Fd(_) => alloc::string::String::from("/"),
            }
        } else if let Ok(p) = StackPathBuf::from_user(name_ptr) {
            let current = current_process();
            let root_fs_arc = current.root_fs();
            let root_fs = root_fs_arc.lock_no_irq();
            let opened_files = current.opened_files_no_irq();
            let follow = (flags & 0x100) == 0; // AT_SYMLINK_FOLLOW = default
            match root_fs.lookup_path_at(&opened_files, &dirfd, p.as_path(), follow) {
                Ok(_comp) => {
                    // Build the full path from the resolved component.
                    // For simplicity, use the input path.
                    alloc::string::String::from(p.as_path().as_str())
                }
                Err(_) => return Err(Errno::ENOENT.into()),
            }
        } else {
            return Err(Errno::EFAULT.into());
        };

        // Look up the mount ID from our mount table.
        let mount_id = MountTable::mount_id_for_path(&full_path);

        // Write mount_id to user space.
        mount_id_ptr.write::<i32>(&mount_id)?;

        // Write a minimal file handle (8 bytes, type=0).
        // We only need to return a valid handle so systemd can compare mount IDs.
        if user_handle.handle_bytes < 8 {
            // Buffer too small — tell the caller how much space we need.
            let needed = FileHandle { handle_bytes: 8, handle_type: 0 };
            handle_ptr.write(&needed)?;
            // EOVERFLOW = 75 on Linux. Use EINVAL as approximation
            // (glibc handles the EOVERFLOW → retry with larger buffer).
            return Err(Errno::EINVAL.into());
        }

        let result = FileHandle { handle_bytes: 8, handle_type: 0 };
        handle_ptr.write(&result)?;
        // Write 8 bytes of handle data (inode number as handle).
        let handle_data_ptr = UserVAddr::new_nonnull(
            handle_ptr.value() + core::mem::size_of::<FileHandle>()
        )?;
        handle_data_ptr.write::<u64>(&(mount_id as u64))?;

        Ok(0)
    }
}
