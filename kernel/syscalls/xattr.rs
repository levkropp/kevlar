// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Extended attribute (xattr) syscalls.
//!
//! Stores xattrs in a global in-memory table keyed by (dev, inode_no).
//! This works for all filesystem types (tmpfs, initramfs, ext4).
use alloc::string::String;
use alloc::vec::Vec;
use hashbrown::HashMap;
use kevlar_platform::address::UserVAddr;
use kevlar_platform::spinlock::SpinLock;

use crate::fs::opened_file::Fd;
use crate::fs::path::Path;
use crate::prelude::*;
use crate::process::current_process;
use crate::result::{Errno, Error, Result};
use crate::syscalls::SyscallHandler;

const XATTR_NAME_MAX: usize = 255;
const XATTR_SIZE_MAX: usize = 65536;
const XATTR_LIST_MAX: usize = 65536;

type InodeKey = (usize, u64); // (dev_id, inode_no)
type XattrMap = HashMap<String, Vec<u8>>;

static XATTR_TABLE: SpinLock<Option<HashMap<InodeKey, XattrMap>>> = SpinLock::new(None);

fn xattr_table() -> &'static SpinLock<Option<HashMap<InodeKey, XattrMap>>> {
    &XATTR_TABLE
}

/// Read a NUL-terminated xattr name from userspace.
fn read_name(name_ptr: usize) -> Result<String> {
    let uaddr = UserVAddr::new_nonnull(name_ptr)?;
    let mut buf = [0u8; XATTR_NAME_MAX + 1];
    let len = uaddr.read_cstr(&mut buf)?;
    if len == 0 {
        return Err(Error::new(Errno::ERANGE));
    }
    String::from_utf8(buf[..len].to_vec())
        .map_err(|_| Error::new(Errno::EINVAL))
}

/// Read xattr value from userspace.
fn read_value(value_ptr: usize, size: usize) -> Result<Vec<u8>> {
    if size > XATTR_SIZE_MAX {
        return Err(Error::new(Errno::E2BIG));
    }
    let uaddr = UserVAddr::new_nonnull(value_ptr)?;
    let mut buf = vec![0u8; size];
    uaddr.read_bytes(&mut buf)?;
    Ok(buf)
}

/// Get the inode key for a path.
fn inode_key_for_path(path: &Path, follow_symlinks: bool) -> Result<InodeKey> {
    let current = current_process();
    let root_fs = current.root_fs();
    let inode = if follow_symlinks {
        root_fs.lock_no_irq().lookup(path)?
    } else {
        root_fs.lock_no_irq().lookup_no_symlink_follow(path)?
    };
    let stat = inode.stat()?;
    Ok((stat.dev.as_usize(), stat.inode_no.as_u64()))
}

/// Get the inode key for a file descriptor.
fn inode_key_for_fd(fd: Fd) -> Result<InodeKey> {
    let opened_file = current_process().get_opened_file_by_fd(fd)?;
    let stat = opened_file.inode().stat()?;
    Ok((stat.dev.as_usize(), stat.inode_no.as_u64()))
}

// ── setxattr family ──

fn do_setxattr(key: InodeKey, name: String, value: Vec<u8>, flags: i32) -> Result<()> {
    const XATTR_CREATE: i32 = 1;
    const XATTR_REPLACE: i32 = 2;

    let mut guard = xattr_table().lock_no_irq();
    let table = guard.get_or_insert_with(HashMap::new);
    let attrs = table.entry(key).or_insert_with(HashMap::new);

    if flags & XATTR_CREATE != 0 && attrs.contains_key(&name) {
        return Err(Error::new(Errno::EEXIST));
    }
    if flags & XATTR_REPLACE != 0 && !attrs.contains_key(&name) {
        return Err(Error::new(Errno::ENODATA));
    }

    attrs.insert(name, value);
    Ok(())
}

// ── getxattr family ──

fn do_getxattr(key: InodeKey, name: &str, value_ptr: usize, size: usize) -> Result<isize> {
    let guard = xattr_table().lock_no_irq();
    let table = match guard.as_ref() {
        Some(t) => t,
        None => return Err(Error::new(Errno::ENODATA)),
    };
    let attrs = table.get(&key).ok_or_else(|| Error::new(Errno::ENODATA))?;
    let value = attrs.get(name).ok_or_else(|| Error::new(Errno::ENODATA))?;

    if size == 0 {
        // Return the size needed.
        return Ok(value.len() as isize);
    }
    if value.len() > size {
        return Err(Error::new(Errno::ERANGE));
    }

    let uaddr = UserVAddr::new_nonnull(value_ptr)?;
    uaddr.write_bytes(value)?;
    Ok(value.len() as isize)
}

// ── listxattr family ──

fn do_listxattr(key: InodeKey, list_ptr: usize, size: usize) -> Result<isize> {
    let guard = xattr_table().lock_no_irq();
    let table = match guard.as_ref() {
        Some(t) => t,
        None => return Ok(0), // No xattrs at all
    };
    let attrs = match table.get(&key) {
        Some(a) => a,
        None => return Ok(0),
    };

    // Build NUL-separated list of names.
    let mut total_len = 0usize;
    for name in attrs.keys() {
        total_len += name.len() + 1; // +1 for NUL
    }

    if size == 0 {
        return Ok(total_len as isize);
    }
    if total_len > size {
        return Err(Error::new(Errno::ERANGE));
    }

    let uaddr = UserVAddr::new_nonnull(list_ptr)?;
    let mut offset = 0;
    for name in attrs.keys() {
        let bytes = name.as_bytes();
        uaddr.add(offset).write_bytes(bytes)?;
        offset += bytes.len();
        uaddr.add(offset).write_bytes(&[0])?; // NUL terminator
        offset += 1;
    }
    Ok(total_len as isize)
}

// ── removexattr family ──

fn do_removexattr(key: InodeKey, name: &str) -> Result<()> {
    let mut guard = xattr_table().lock_no_irq();
    let table = match guard.as_mut() {
        Some(t) => t,
        None => return Err(Error::new(Errno::ENODATA)),
    };
    let attrs = table.get_mut(&key).ok_or_else(|| Error::new(Errno::ENODATA))?;
    if attrs.remove(name).is_none() {
        return Err(Error::new(Errno::ENODATA));
    }
    if attrs.is_empty() {
        table.remove(&key);
    }
    Ok(())
}

// ── Syscall handlers ──

use super::StackPathBuf;

impl<'a> SyscallHandler<'a> {
    // setxattr / lsetxattr / fsetxattr
    pub fn sys_setxattr(&mut self, path_ptr: usize, name_ptr: usize, value_ptr: usize, size: usize, flags: i32, follow: bool) -> Result<isize> {
        let spb = StackPathBuf::from_user(path_ptr)?;
        let key = inode_key_for_path(spb.as_path(), follow)?;
        let name = read_name(name_ptr)?;
        let value = read_value(value_ptr, size)?;
        do_setxattr(key, name, value, flags)?;
        Ok(0)
    }

    pub fn sys_fsetxattr(&mut self, fd: Fd, name_ptr: usize, value_ptr: usize, size: usize, flags: i32) -> Result<isize> {
        let key = inode_key_for_fd(fd)?;
        let name = read_name(name_ptr)?;
        let value = read_value(value_ptr, size)?;
        do_setxattr(key, name, value, flags)?;
        Ok(0)
    }

    // getxattr / lgetxattr / fgetxattr
    pub fn sys_getxattr(&mut self, path_ptr: usize, name_ptr: usize, value_ptr: usize, size: usize, follow: bool) -> Result<isize> {
        let spb = StackPathBuf::from_user(path_ptr)?;
        let key = inode_key_for_path(spb.as_path(), follow)?;
        let name = read_name(name_ptr)?;
        do_getxattr(key, &name, value_ptr, size)
    }

    pub fn sys_fgetxattr(&mut self, fd: Fd, name_ptr: usize, value_ptr: usize, size: usize) -> Result<isize> {
        let key = inode_key_for_fd(fd)?;
        let name = read_name(name_ptr)?;
        do_getxattr(key, &name, value_ptr, size)
    }

    // listxattr / llistxattr / flistxattr
    pub fn sys_listxattr(&mut self, path_ptr: usize, list_ptr: usize, size: usize, follow: bool) -> Result<isize> {
        let spb = StackPathBuf::from_user(path_ptr)?;
        let key = inode_key_for_path(spb.as_path(), follow)?;
        do_listxattr(key, list_ptr, size)
    }

    pub fn sys_flistxattr(&mut self, fd: Fd, list_ptr: usize, size: usize) -> Result<isize> {
        let key = inode_key_for_fd(fd)?;
        do_listxattr(key, list_ptr, size)
    }

    // removexattr / lremovexattr / fremovexattr
    pub fn sys_removexattr(&mut self, path_ptr: usize, name_ptr: usize, follow: bool) -> Result<isize> {
        let spb = StackPathBuf::from_user(path_ptr)?;
        let key = inode_key_for_path(spb.as_path(), follow)?;
        let name = read_name(name_ptr)?;
        do_removexattr(key, &name)?;
        Ok(0)
    }

    pub fn sys_fremovexattr(&mut self, fd: Fd, name_ptr: usize) -> Result<isize> {
        let key = inode_key_for_fd(fd)?;
        let name = read_name(name_ptr)?;
        do_removexattr(key, &name)?;
        Ok(0)
    }
}
