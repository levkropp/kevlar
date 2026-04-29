// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! `KabiDirectory` and `KabiFile` — Kevlar VFS adapters for Linux
//! kABI-mounted filesystems (Phase 5).
//!
//! After `kabi_mount_filesystem` succeeds, `KabiFileSystem::root_dir()`
//! returns a `KabiDirectory` rooted at `super_block->s_root`.
//! Userspace's `mount(2)` then proceeds normally; subsequent
//! `getdents`, `stat`, `open` etc. land here.
//!
//! `KabiDirectory` drives erofs's compiled
//! `inode->i_fop->iterate_shared(file, ctx)` via our `kabi_filldir`
//! actor to enumerate entries, and
//! `inode->i_op->lookup(parent, child_dentry, 0)` to resolve names.
//!
//! `KabiFile` is a Phase 5 placeholder that exposes `stat()` but
//! returns `ENOSYS` from `read()`.  Phase 6 wires `read()` to
//! `read_folio` so file contents come back to userspace.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};

use kevlar_platform::spinlock::SpinLock;
use kevlar_vfs::inode::{
    DirEntry, Directory, FileLike, FileType, INode, INodeNo, OpenOptions,
};
use kevlar_vfs::result::{Errno, Error, Result as VfsResult};
use kevlar_vfs::stat::{DevId, FileMode, FileSize, NLink, Stat};
use kevlar_vfs::user_buffer::{UserBuffer, UserBufferMut};

use super::struct_layouts as fl;

// ── Linux 7.0 struct offsets used here (cross-checked against
//    build/linux-src.vanilla-v7.0/include/linux/fs.h + erofs.ko
//    disasm) ───────────────────────────────────────────────────────

/// Offset of `i_fop` (file_operations *) in struct inode.
/// Linux 7.0 inode is large; CONFIG-dependent.  Per Phase 4 disasm
/// notes (erofs_fill_inode storing at +0x160), this lands at +352.
const INODE_I_FOP_OFF: usize = 352;

/// Offset of `iterate_shared` in struct file_operations.
/// Layout: owner+0(8), fop_flags+8(4+pad), then 7 fn ptrs:
/// llseek, read, write, read_iter, write_iter, iopoll,
/// iterate_shared.  iterate_shared = +16 + 6*8 = +64.
const FOP_ITERATE_SHARED_OFF: usize = 64;

/// Offset of `lookup` in struct inode_operations — first field.
const IOP_LOOKUP_OFF: usize = 0;

/// Linux mode bits we care about.
const S_IFMT: u32 = 0o170000;
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const S_IFLNK: u32 = 0o120000;

// ── KabiDirectory ───────────────────────────────────────────────

/// Kevlar `Directory` adapter wrapping a Linux dentry/inode pair.
pub struct KabiDirectory {
    dentry: usize,
    inode: usize,
    sb: usize,
    dev_id: usize,
    name: String,
    /// Cached dirents — populated on first `readdir(0)` call.
    /// Subsequent calls index into this vec.
    cached: SpinLock<Option<Vec<DirEntry>>>,
}

unsafe impl Send for KabiDirectory {}
unsafe impl Sync for KabiDirectory {}

impl core::fmt::Debug for KabiDirectory {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("KabiDirectory")
            .field("name", &self.name)
            .field("dentry", &(self.dentry as *const ()))
            .field("inode", &(self.inode as *const ()))
            .finish()
    }
}

impl KabiDirectory {
    pub fn new(
        dentry: usize, inode: usize, sb: usize,
        dev_id: usize, name: String,
    ) -> Self {
        KabiDirectory {
            dentry, inode, sb, dev_id, name,
            cached: SpinLock::new(None),
        }
    }

    fn read_inode_field<T: Copy>(&self, off: usize) -> T {
        unsafe { *((self.inode as *const u8).add(off) as *const T) }
    }

    /// Drive `inode->i_fop->iterate_shared(file, ctx)` once, capture
    /// every entry the .ko emits, return them as a Vec.
    fn fetch_entries(&self) -> VfsResult<Vec<DirEntry>> {
        // 1. Read i_fop and iterate_shared.
        let i_fop: usize = self.read_inode_field(INODE_I_FOP_OFF);
        if i_fop == 0 {
            log::warn!("kabi: KabiDirectory({}): i_fop is null", self.name);
            return Err(Error::new(Errno::ENOTDIR));
        }
        let iterate_ptr: usize = unsafe {
            *((i_fop as *const u8).add(FOP_ITERATE_SHARED_OFF)
                as *const usize)
        };
        if iterate_ptr == 0 {
            log::warn!("kabi: KabiDirectory({}): iterate_shared is null",
                       self.name);
            return Err(Error::new(Errno::ENOTDIR));
        }

        // 2. Synthesise a struct file for the directory.  Reuse the
        //    layout pattern from fs_synth: 256 bytes, populate
        //    f_inode and f_mapping.
        let file = super::alloc::kzalloc(fl::FILE_SIZE, 0) as *mut u8;
        if file.is_null() {
            return Err(Error::new(Errno::ENOMEM));
        }
        let i_mapping: usize = self.read_inode_field(fl::INODE_I_MAPPING_OFF);
        unsafe {
            *(file.add(fl::FILE_F_MAPPING_OFF) as *mut usize) = i_mapping;
            *(file.add(fl::FILE_F_INODE_OFF) as *mut usize) = self.inode;
        }

        // 3. Allocate dir_context (24 bytes): actor, pos, count,
        //    dt_flags_mask.
        let ctx = super::alloc::kzalloc(32, 0) as *mut u8;
        if ctx.is_null() {
            super::alloc::kfree(file as *mut c_void);
            return Err(Error::new(Errno::ENOMEM));
        }
        unsafe {
            *(ctx.add(0) as *mut usize) = kabi_filldir as usize;
            *(ctx.add(8) as *mut i64) = 0;       // pos
            *(ctx.add(16) as *mut i32) = i32::MAX; // count
            *(ctx.add(20) as *mut u32) = 0;      // dt_flags_mask
        }

        // 4. Lock the global filldir capture buffer (single-threaded
        //    in v1; only one active iterate at a time).
        {
            let mut buf = FILLDIR_BUFFER.lock();
            *buf = Some(Vec::new());
        }

        log::info!(
            "kabi: KabiDirectory({}).fetch_entries: dispatching \
             iterate_shared(file={:p}, ctx={:p})",
            self.name, file, ctx,
        );

        // 5. SCS-wrap the call.
        let rc = super::loader::call_with_scs_2(
            iterate_ptr as *const (),
            file as usize,
            ctx as usize,
        ) as i32;
        log::info!(
            "kabi: KabiDirectory({}).fetch_entries: iterate_shared \
             returned {}",
            self.name, rc,
        );

        super::alloc::kfree(ctx as *mut c_void);
        super::alloc::kfree(file as *mut c_void);

        // 6. Take the captured entries.
        let entries = FILLDIR_BUFFER
            .lock()
            .take()
            .unwrap_or_else(Vec::new);

        if rc < 0 {
            return Err(Error::new(Errno::EIO));
        }
        Ok(entries)
    }
}

impl Directory for KabiDirectory {
    fn lookup(&self, name: &str) -> VfsResult<INode> {
        // Phase 5e — wrap inode->i_op->lookup(parent, child_dentry, 0).
        // Allocate child dentry, fill name + parent + sb, dispatch.
        // Phase 5f wraps the result as INode::FileLike(KabiFile) /
        // INode::Directory(KabiDirectory).
        log::warn!(
            "kabi: KabiDirectory({}).lookup({:?}) — not yet implemented",
            self.name, name,
        );
        Err(Error::new(Errno::ENOSYS))
    }

    fn create_file(
        &self, _name: &str, _mode: FileMode,
        _uid: kevlar_vfs::stat::UId, _gid: kevlar_vfs::stat::GId,
    ) -> VfsResult<INode> {
        Err(Error::new(Errno::EROFS))
    }

    fn create_dir(
        &self, _name: &str, _mode: FileMode,
        _uid: kevlar_vfs::stat::UId, _gid: kevlar_vfs::stat::GId,
    ) -> VfsResult<INode> {
        Err(Error::new(Errno::EROFS))
    }

    fn link(&self, _name: &str, _link_to: &INode) -> VfsResult<()> {
        Err(Error::new(Errno::EROFS))
    }

    fn stat(&self) -> VfsResult<Stat> {
        // Read i_mode (u16 at +0), i_size (i64 at +80), i_ino
        // (usize at +64).
        let i_mode: u16 = self.read_inode_field(fl::INODE_I_MODE_OFF);
        let i_size: i64 = self.read_inode_field(fl::INODE_I_SIZE_OFF);
        let i_ino: usize = self.read_inode_field(fl::INODE_I_INO_OFF);
        Ok(Stat {
            dev: DevId::new(self.dev_id),
            inode_no: INodeNo::new(i_ino),
            mode: FileMode::new(i_mode as u32),
            size: FileSize(i_size as isize),
            nlink: NLink::new(1),
            ..Stat::zeroed()
        })
    }

    fn dev_id(&self) -> usize {
        self.dev_id
    }

    fn readdir(&self, index: usize) -> VfsResult<Option<DirEntry>> {
        // Synthesise "." and ".." for indices 0 and 1.
        if index == 0 {
            let ino: usize = self.read_inode_field(fl::INODE_I_INO_OFF);
            return Ok(Some(DirEntry {
                inode_no: INodeNo::new(ino),
                file_type: FileType::Directory,
                name: String::from("."),
            }));
        }
        if index == 1 {
            let ino: usize = self.read_inode_field(fl::INODE_I_INO_OFF);
            return Ok(Some(DirEntry {
                inode_no: INodeNo::new(ino),
                file_type: FileType::Directory,
                name: String::from(".."),
            }));
        }

        // Cache the .ko-emitted entries on first non-./..  call.
        let mut cached = self.cached.lock();
        if cached.is_none() {
            drop(cached);
            let entries = self.fetch_entries()?;
            *self.cached.lock() = Some(entries);
            cached = self.cached.lock();
        }

        let entries = cached.as_ref().expect("just populated");
        let real_idx = index - 2;
        if real_idx >= entries.len() {
            return Ok(None);
        }
        let de = &entries[real_idx];
        Ok(Some(DirEntry {
            inode_no: de.inode_no,
            file_type: de.file_type,
            name: de.name.clone(),
        }))
    }
}

// ── KabiFile (Phase 5f placeholder; Phase 6 makes read() real) ──

pub struct KabiFile {
    inode: usize,
    sb: usize,
    name: String,
}

unsafe impl Send for KabiFile {}
unsafe impl Sync for KabiFile {}

impl core::fmt::Debug for KabiFile {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("KabiFile")
            .field("name", &self.name)
            .field("inode", &(self.inode as *const ()))
            .finish()
    }
}

impl KabiFile {
    pub fn new(inode: usize, sb: usize, name: String) -> Self {
        KabiFile { inode, sb, name }
    }

    fn read_inode_field<T: Copy>(&self, off: usize) -> T {
        unsafe { *((self.inode as *const u8).add(off) as *const T) }
    }
}

impl FileLike for KabiFile {
    fn stat(&self) -> VfsResult<Stat> {
        let i_mode: u16 = self.read_inode_field(fl::INODE_I_MODE_OFF);
        let i_size: i64 = self.read_inode_field(fl::INODE_I_SIZE_OFF);
        let i_ino: usize = self.read_inode_field(fl::INODE_I_INO_OFF);
        Ok(Stat {
            dev: DevId::new(0),
            inode_no: INodeNo::new(i_ino),
            mode: FileMode::new(i_mode as u32),
            size: FileSize(i_size as isize),
            nlink: NLink::new(1),
            ..Stat::zeroed()
        })
    }

    fn read(
        &self, _offset: usize, _buf: UserBufferMut<'_>,
        _options: &OpenOptions,
    ) -> VfsResult<usize> {
        log::warn!("kabi: KabiFile({}).read — Phase 6 lands this", self.name);
        Err(Error::new(Errno::ENOSYS))
    }

    fn write(
        &self, _offset: usize, _buf: UserBuffer<'_>,
        _options: &OpenOptions,
    ) -> VfsResult<usize> {
        Err(Error::new(Errno::EROFS))
    }
}

// ── filldir capture ─────────────────────────────────────────────

/// Global capture buffer for entries emitted by erofs's
/// iterate_shared callback chain.  Single-mount v1 — populated by
/// `KabiDirectory::fetch_entries` before dispatching, drained
/// after.  Filled by `kabi_filldir`.
pub(super) static FILLDIR_BUFFER: SpinLock<Option<Vec<DirEntry>>> =
    SpinLock::new(None);

/// `filldir_t` shim — invoked by erofs's iterate_shared callback
/// for each entry.  Pushes a `DirEntry` into the active capture
/// buffer; returns 1 to continue, 0 to stop.
///
/// Linux signature:
///   bool (*filldir_t)(struct dir_context *ctx, const char *name,
///                     int len, loff_t pos, u64 ino, unsigned dt_type)
///
/// Note: bool is u8 in Rust extern "C" but on arm64 the ABI passes
/// it in w0 (a 32-bit register).  Using i32 with 0/1 is safe.
#[unsafe(no_mangle)]
pub extern "C" fn kabi_filldir(
    _ctx: *mut c_void,
    name: *const u8,
    len: i32,
    _pos: i64,
    ino: u64,
    dt_type: u32,
) -> i32 {
    if name.is_null() || len <= 0 {
        return 1; // keep going
    }
    let bytes = unsafe { core::slice::from_raw_parts(name, len as usize) };
    let name_str = match core::str::from_utf8(bytes) {
        Ok(s) => alloc::string::String::from(s),
        Err(_) => return 1,
    };
    // Filter "." and ".." — KabiDirectory::readdir synthesises
    // those at indices 0/1 from the inode's i_ino, matching the
    // tmpfs convention.  Erofs emits them too; skip the dups.
    if name_str == "." || name_str == ".." {
        return 1;
    }
    let file_type = match dt_type {
        4 => FileType::Directory,
        8 => FileType::Regular,
        10 => FileType::Link,
        12 => FileType::Socket,
        _  => FileType::Regular,
    };
    let de = DirEntry {
        inode_no: INodeNo::new(ino as usize),
        file_type,
        name: name_str.clone(),
    };
    log::info!(
        "kabi: filldir captured: name={:?} ino={} dt_type={}",
        name_str, ino, dt_type,
    );
    if let Some(buf) = FILLDIR_BUFFER.lock().as_mut() {
        buf.push(de);
    }
    1 // continue
}
