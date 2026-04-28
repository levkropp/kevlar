// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Char-device registration shims for K4.
//!
//! Modules call `register_chrdev(0, "name", &fops)` (or the
//! finer-grained `alloc_chrdev_region` + `cdev_init` + `cdev_add`
//! pair) and a `/dev/<name>` node appears in Kevlar's existing
//! devfs root, backed by the `KabiCharDevFile` adapter that
//! dispatches FileLike trait methods through the module-supplied
//! `file_operations` table.

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ffi::{c_char, c_void};
use core::fmt;
use core::sync::atomic::{AtomicU32, Ordering};

use kevlar_platform::spinlock::SpinLock;
use kevlar_vfs::inode::FileLike;
use kevlar_vfs::result::{Errno as VfsErrno, Error as VfsError, Result as VfsResult};
use kevlar_vfs::stat::{DevId, FileMode, Stat, S_IFCHR};
use kevlar_vfs::user_buffer::{
    UserBufReader, UserBufWriter, UserBuffer, UserBufferMut,
};

use crate::fs::devfs::{DevFs, DEV_FS};
use crate::fs::opened_file::OpenOptions;
use kevlar_vfs::file_system::FileSystem;
use crate::kabi::fops::{FileOperationsShim, FileShim, InodeShim};
use crate::ksym;

pub type DevT = u32;

#[inline]
pub fn major_of(d: DevT) -> u32 {
    (d >> 8) & 0xff_ffff
}
#[inline]
pub fn minor_of(d: DevT) -> u32 {
    d & 0xff
}
#[inline]
pub fn make_dev(major: u32, minor: u32) -> DevT {
    (minor & 0xff) | (major << 8)
}

#[repr(C)]
pub struct CdevShim {
    pub _kevlar_inner: *mut c_void,
    pub ops: *const FileOperationsShim,
    pub dev: DevT,
    pub count: u32,
}

struct CharDevEntry {
    name: String,
    major: u32,
    minor: u32,
    count: u32,
    fops: *const FileOperationsShim,
}

// SAFETY: fops pointers are kernel-stable (the module image never
// moves once loaded; modules aren't unloaded in K1-K4).
unsafe impl Send for CharDevEntry {}
unsafe impl Sync for CharDevEntry {}

static CHRDEV_REGISTRY: SpinLock<Vec<CharDevEntry>> = SpinLock::new(Vec::new());

/// Dynamic-major allocation begins here (Linux experimental
/// LOCAL/EXPERIMENTAL range starts at 240).
static NEXT_DYNAMIC_MAJOR: AtomicU32 = AtomicU32::new(240);

#[inline]
fn alloc_dynamic_major() -> u32 {
    NEXT_DYNAMIC_MAJOR.fetch_add(1, Ordering::Relaxed)
}

#[inline]
unsafe fn c_str_to_string(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    while len < 256 && unsafe { *p.add(len) } != 0 {
        len += 1;
    }
    let bytes = unsafe { core::slice::from_raw_parts(p as *const u8, len) };
    String::from_utf8_lossy(bytes).into_owned()
}

// ── Linux-shape API ─────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn alloc_chrdev_region(
    out_dev: *mut DevT,
    baseminor: u32,
    _count: u32,
    _name: *const c_char,
) -> i32 {
    if out_dev.is_null() {
        return -22;
    }
    let major = alloc_dynamic_major();
    unsafe {
        *out_dev = make_dev(major, baseminor);
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn register_chrdev_region(
    _first: DevT,
    _count: u32,
    _name: *const c_char,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn unregister_chrdev_region(_first: DevT, _count: u32) {
    // K4: leak the registry entry.
}

#[unsafe(no_mangle)]
pub extern "C" fn cdev_init(
    cdev: *mut CdevShim,
    fops: *const FileOperationsShim,
) {
    if cdev.is_null() {
        return;
    }
    unsafe {
        (*cdev).ops = fops;
        (*cdev).count = 0;
        (*cdev)._kevlar_inner = core::ptr::null_mut();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cdev_add(
    cdev: *mut CdevShim,
    dev: DevT,
    count: u32,
) -> i32 {
    if cdev.is_null() {
        return -22;
    }
    unsafe {
        (*cdev).dev = dev;
        (*cdev).count = count;
    }
    let fops = unsafe { (*cdev).ops };
    if fops.is_null() {
        return -22;
    }
    install_chrdev(major_of(dev), minor_of(dev), count, "cdev", fops);
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn cdev_del(_cdev: *mut CdevShim) {
    // K4: registry leak — modules don't unload.
}

/// `register_chrdev(0, name, fops)` allocates a major dynamically
/// and installs `/dev/<name>`.  Returns the allocated major
/// (positive) or a negative errno.
#[unsafe(no_mangle)]
pub extern "C" fn register_chrdev(
    major: u32,
    name: *const c_char,
    fops: *const FileOperationsShim,
) -> i32 {
    if name.is_null() || fops.is_null() {
        return -22;
    }
    let m = if major == 0 { alloc_dynamic_major() } else { major };
    let name_str = unsafe { c_str_to_string(name) };
    install_chrdev(m, 0, 256, &name_str, fops);
    m as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn unregister_chrdev(_major: u32, _name: *const c_char) {
    // K4: registry leak.
}

ksym!(alloc_chrdev_region);
ksym!(register_chrdev_region);
ksym!(unregister_chrdev_region);
ksym!(cdev_init);
ksym!(cdev_add);
ksym!(cdev_del);
ksym!(register_chrdev);
ksym!(unregister_chrdev);

// ── Installation: /dev/<name> + registry entry ─────────────────

fn install_chrdev(
    major: u32,
    minor: u32,
    count: u32,
    name: &str,
    fops: *const FileOperationsShim,
) {
    let rdev = make_dev(major, minor);
    let entry = CharDevEntry {
        name: name.to_string(),
        major,
        minor,
        count,
        fops,
    };
    CHRDEV_REGISTRY.lock().push(entry);

    let adapter: Arc<dyn FileLike> = Arc::new(KabiCharDevFile {
        fops,
        name: name.to_string(),
        rdev,
    });
    let devfs: &DevFs = &*DEV_FS;
    devfs.add_runtime_file(name, adapter);
    log::info!(
        "kabi: /dev/{} registered (major={}, minor={}, rdev={:#x})",
        name, major, minor, rdev
    );
}

/// Install a char device at /dev/<subdir>/<name>.  Used by the
/// DRM registration path (K20+) for /dev/dri/cardN.
pub fn install_chrdev_in_subdir(
    major: u32,
    minor: u32,
    count: u32,
    subdir: &str,
    name: &str,
    fops: *const FileOperationsShim,
) {
    let rdev = make_dev(major, minor);
    let registry_name = format!("{}/{}", subdir, name);
    let entry = CharDevEntry {
        name: registry_name.clone(),
        major,
        minor,
        count,
        fops,
    };
    CHRDEV_REGISTRY.lock().push(entry);

    let adapter: Arc<dyn FileLike> = Arc::new(KabiCharDevFile {
        fops,
        name: registry_name,
        rdev,
    });
    let devfs: &DevFs = &*DEV_FS;
    devfs.add_runtime_file_in_subdir(subdir, name, adapter);
    log::info!(
        "kabi: /dev/{}/{} registered (major={}, minor={}, rdev={:#x})",
        subdir, name, major, minor, rdev
    );
}

// ── FileLike adapter ────────────────────────────────────────────

struct KabiCharDevFile {
    fops: *const FileOperationsShim,
    name: String,
    rdev: u32,
}

impl fmt::Debug for KabiCharDevFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KabiCharDevFile").field("name", &self.name).finish()
    }
}

// SAFETY: the fops pointer references kernel-loaded module data;
// modules aren't unloaded in K1-K4.  Methods on FileLike are
// `&self` and Send/Sync compatible with our usage pattern.
unsafe impl Send for KabiCharDevFile {}
unsafe impl Sync for KabiCharDevFile {}

fn errno_from_neg(_rc: isize) -> VfsError {
    // K4 collapses all negative-rc errors into EIO; finer-grained
    // errno mapping arrives when a future milestone exposes a real
    // i32→Errno table.
    VfsError::new(VfsErrno::EIO)
}

impl FileLike for KabiCharDevFile {
    fn open(
        &self,
        _options: &OpenOptions,
    ) -> VfsResult<Option<Arc<dyn FileLike>>> {
        if let Some(open_fn) = unsafe { (*self.fops).open } {
            let mut inode = InodeShim {
                _kevlar_inner: core::ptr::null_mut(),
                i_rdev: self.rdev,
                _pad: 0,
                i_size: 0,
            };
            let mut filp = FileShim {
                _kevlar_inner: core::ptr::null_mut(),
                private_data: core::ptr::null_mut(),
                f_pos: 0,
                f_flags: 0,
                _pad: 0,
            };
            let rc = open_fn(&mut inode, &mut filp);
            if rc < 0 {
                return Err(errno_from_neg(rc as isize));
            }
        }
        Ok(None)
    }

    fn stat(&self) -> VfsResult<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFCHR | 0o666),
            rdev: DevId::new(self.rdev as usize),
            ..Stat::zeroed()
        })
    }

    fn read(
        &self,
        offset: usize,
        mut buf: UserBufferMut<'_>,
        _options: &OpenOptions,
    ) -> VfsResult<usize> {
        let read_fn = match unsafe { (*self.fops).read } {
            Some(f) => f,
            None => return Err(VfsError::new(VfsErrno::EBADF)),
        };
        if buf.len() == 0 {
            return Ok(0);
        }
        let mut tmp: alloc::vec::Vec<u8> = alloc::vec![0u8; buf.len()];
        let mut filp = FileShim {
            _kevlar_inner: core::ptr::null_mut(),
            private_data: core::ptr::null_mut(),
            f_pos: offset as i64,
            f_flags: 0,
            _pad: 0,
        };
        let n = read_fn(&mut filp, tmp.as_mut_ptr(), tmp.len(), &raw mut filp.f_pos);
        if n < 0 {
            return Err(errno_from_neg(n));
        }
        let n = n as usize;
        if n > tmp.len() {
            return Err(VfsError::new(VfsErrno::EIO));
        }
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&tmp[..n])?;
        Ok(n)
    }

    fn write(
        &self,
        offset: usize,
        buf: UserBuffer<'_>,
        _options: &OpenOptions,
    ) -> VfsResult<usize> {
        let write_fn = match unsafe { (*self.fops).write } {
            Some(f) => f,
            None => return Err(VfsError::new(VfsErrno::EBADF)),
        };
        if buf.len() == 0 {
            return Ok(0);
        }
        let mut tmp: alloc::vec::Vec<u8> = alloc::vec![0u8; buf.len()];
        // Pull the user buffer into the kernel scratch.
        let mut reader = UserBufReader::from(buf);
        let nread = reader.read_bytes(&mut tmp)?;
        let mut filp = FileShim {
            _kevlar_inner: core::ptr::null_mut(),
            private_data: core::ptr::null_mut(),
            f_pos: offset as i64,
            f_flags: 0,
            _pad: 0,
        };
        let n = write_fn(&mut filp, tmp.as_ptr(), nread, &raw mut filp.f_pos);
        if n < 0 {
            return Err(errno_from_neg(n));
        }
        Ok(n as usize)
    }

    fn ioctl(&self, cmd: usize, arg: usize) -> VfsResult<isize> {
        let f = match unsafe { (*self.fops).unlocked_ioctl } {
            Some(f) => f,
            None => return Err(VfsError::new(VfsErrno::ENOTTY)),
        };
        let mut filp = FileShim {
            _kevlar_inner: core::ptr::null_mut(),
            private_data: core::ptr::null_mut(),
            f_pos: 0,
            f_flags: 0,
            _pad: 0,
        };
        let rc = f(&mut filp, cmd as u32, arg);
        if rc < 0 {
            return Err(errno_from_neg(rc));
        }
        Ok(rc)
    }
}

// Reference fields so the compiler doesn't drop them as unused.
#[allow(dead_code)]
fn _unused(_: &CharDevEntry) {}
impl CharDevEntry {
    #[allow(dead_code)]
    fn touch(&self) {
        let _ = (&self.major, &self.minor, &self.count, &self.fops);
    }
}

/// Kernel-side smoke test: open `/dev/<name>` via the registered
/// adapter, read up to `buf.len()` bytes into `buf`, then call
/// release.  Returns the number of bytes read or an error.  Used
/// by `kernel/main.rs` to verify the K4 dispatch path end-to-end
/// without needing a userspace harness.
pub fn read_dev_for_test(name: &str, buf: &mut [u8]) -> VfsResult<usize> {
    use kevlar_vfs::inode::{Directory, INode};
    let devfs: &DevFs = &*DEV_FS;
    let root = devfs.root_dir()?;
    let inode = root.lookup(name)?;
    let file = match inode {
        INode::FileLike(f) => f,
        _ => return Err(VfsError::new(VfsErrno::EISDIR)),
    };
    let opts = OpenOptions::readwrite();
    if let Some(opened) = file.open(&opts)? {
        // open returned a fresh FileLike; use it
        let n = opened.read(0, (&mut buf[..]).into(), &opts)?;
        return Ok(n);
    }
    let n = file.read(0, (&mut buf[..]).into(), &opts)?;
    Ok(n)
}
