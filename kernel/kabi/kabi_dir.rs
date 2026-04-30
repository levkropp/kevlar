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
use kevlar_vfs::user_buffer::{UserBufWriter, UserBuffer, UserBufferMut};

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

/// Map a positive Linux errno to a kevlar_vfs Errno.  Phase 5 v4
/// scope: only the codes erofs's lookup/namei produces.
fn map_errno(errno: i32) -> Error {
    let e = match errno {
        2  => Errno::ENOENT,
        12 => Errno::ENOMEM,
        20 => Errno::ENOTDIR,
        22 => Errno::EINVAL,
        // ENAMETOOLONG (36) is used by erofs but kevlar_vfs's Errno
        // enum maps to EINVAL — close enough for now.
        36 => Errno::EINVAL,
        _  => Errno::EIO,
    };
    Error::new(e)
}

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
        // Phase 5 v4: SCS-wrapped dispatch into
        // `parent_inode->i_op->lookup(parent_inode, child_dentry, 0)`.
        // Erofs's compiled `erofs_lookup` reads dentry->d_name,
        // calls `erofs_namei` which walks the dir blocks (now
        // working since Phase 5 v3's layout-aware read_cache_folio),
        // calls `erofs_iget` for the matching inode, and dispatches
        // through `d_splice_alias` (now real, sets d_inode).
        log::info!("kabi: KabiDirectory({}).lookup({:?}) — dispatching",
                   self.name, name);

        // 1. Allocate child dentry (zeroed).
        let dentry = super::alloc::kzalloc(fl::DENTRY_SIZE,
            super::alloc::__GFP_ZERO);
        if dentry.is_null() {
            return Err(Error::new(Errno::ENOMEM));
        }

        // 2. Allocate name buffer + null terminator.
        let name_buf = super::alloc::kmalloc(name.len() + 1, 0)
            as *mut u8;
        if name_buf.is_null() {
            super::alloc::kfree(dentry);
            return Err(Error::new(Errno::ENOMEM));
        }
        unsafe {
            core::ptr::copy_nonoverlapping(name.as_ptr(), name_buf,
                                           name.len());
            *name_buf.add(name.len()) = 0;
        }

        // 3. Populate d_name qstr (16 bytes at +40):
        //    +0..+4: hash (u32, 0 — erofs computes its own)
        //    +4..+8: len  (u32)
        //    +8..+16: name pointer
        unsafe {
            let qstr = (dentry as *mut u8).add(fl::DENTRY_D_NAME_OFF);
            *(qstr.add(0) as *mut u32) = 0;
            *(qstr.add(4) as *mut u32) = name.len() as u32;
            *(qstr.add(8) as *mut *mut u8) = name_buf;
        }

        // 4. Set d_parent + d_sb.
        unsafe {
            *((dentry as *mut u8).add(fl::DENTRY_D_PARENT_OFF)
                as *mut usize) = self.dentry;
            *((dentry as *mut u8).add(fl::DENTRY_D_SB_OFF)
                as *mut usize) = self.sb;
        }

        // 5. Read parent_inode->i_op->lookup.
        let i_op: usize = unsafe {
            *((self.inode as *const u8)
                .add(fl::INODE_I_OP_OFF) as *const usize)
        };
        if i_op == 0 {
            log::warn!("kabi: lookup: i_op is null");
            unsafe {
                super::alloc::kfree(name_buf as *mut c_void);
                super::alloc::kfree(dentry);
            }
            return Err(Error::new(Errno::ENOSYS));
        }
        let lookup_fn: usize = unsafe { *(i_op as *const usize) };
        if lookup_fn == 0 {
            log::warn!("kabi: lookup: i_op->lookup is null");
            unsafe {
                super::alloc::kfree(name_buf as *mut c_void);
                super::alloc::kfree(dentry);
            }
            return Err(Error::new(Errno::ENOSYS));
        }

        // 6. SCS-wrap the call.
        log::info!("kabi: lookup: dispatching i_op->lookup={:#x} \
                    inode={:#x} dentry={:p} name={:?}",
                   lookup_fn, self.inode, dentry, name);
        let result_raw = super::loader::call_with_scs_3(
            lookup_fn as *const (),
            self.inode,
            dentry as usize,
            0, // flags
        );
        log::info!("kabi: lookup: returned {:#x}", result_raw);

        // 7. Decode result.
        let target_dentry: *mut c_void = if result_raw == 0 {
            // NULL — caller uses input dentry.  d_splice_alias has
            // already set its d_inode (or left it null for negative).
            dentry
        } else if result_raw < 0 && result_raw >= -4095 {
            // ERR_PTR — forward the errno.
            let errno = -(result_raw as i32);
            unsafe {
                super::alloc::kfree(name_buf as *mut c_void);
                super::alloc::kfree(dentry);
            }
            return Err(map_errno(errno));
        } else {
            // Replacement dentry returned.
            result_raw as *mut c_void
        };

        // 8. Read child_dentry->d_inode.
        let inode: usize = unsafe {
            *((target_dentry as *const u8)
                .add(fl::DENTRY_D_INODE_OFF) as *const usize)
        };
        if inode == 0 {
            // Negative dentry — name not found.
            log::info!("kabi: lookup({:?}): negative dentry", name);
            return Err(Error::new(Errno::ENOENT));
        }

        // 9. Wrap based on i_mode.
        let i_mode: u16 = unsafe {
            *((inode as *const u8)
                .add(fl::INODE_I_MODE_OFF) as *const u16)
        };
        const S_IFMT: u16 = 0o170000;
        const S_IFDIR_K: u16 = 0o040000;
        const S_IFREG_K: u16 = 0o100000;
        let kind = i_mode & S_IFMT;
        log::info!("kabi: lookup({:?}): inode={:#x} i_mode={:#o}",
                   name, inode, i_mode);
        match kind {
            S_IFDIR_K => Ok(INode::Directory(Arc::new(KabiDirectory::new(
                target_dentry as usize, inode, self.sb,
                self.dev_id, self.name.clone(),
            )))),
            S_IFREG_K => Ok(INode::FileLike(Arc::new(KabiFile::new(
                inode, self.sb, self.name.clone(),
            )))),
            _ => {
                log::warn!("kabi: lookup({:?}): unsupported i_mode {:#o}",
                           name, i_mode);
                Err(Error::new(Errno::ENOSYS))
            }
        }
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

    /// Phase 6 path — used for filesystems whose on-disk layout the
    /// kABI side decodes directly (erofs FLAT_PLAIN / FLAT_INLINE).
    fn read_via_inode_meta(
        &self, offset: usize, want: usize,
        buf: UserBufferMut<'_>,
        meta: super::inode_meta::KabiInodeMeta,
    ) -> VfsResult<usize> {
        let mut writer = UserBufWriter::from(buf);
        let mut done = 0usize;
        let mut kbuf = [0u8; 4096];
        while done < want {
            let file_pos = offset + done;
            let page_idx = (file_pos / 4096) as u64;
            let (path, phys_off, inline_shift) =
                super::inode_meta::translate_offset(&meta, page_idx)
                    .ok_or_else(|| Error::new(Errno::EIO))?;
            super::filemap::read_initramfs_at(
                &path, phys_off as usize,
                kbuf.as_mut_ptr(), 4096,
            ).map_err(|_| Error::new(Errno::EIO))?;
            let in_page = file_pos % 4096;
            let src = (inline_shift as usize) + in_page;
            if src >= 4096 {
                break;
            }
            let chunk = (want - done).min(4096 - src);
            if chunk == 0 {
                break;
            }
            writer.write_bytes(&kbuf[src..src + chunk])
                .map_err(|_| Error::new(Errno::EIO))?;
            done += chunk;
        }
        Ok(done)
    }

    /// Phase 13 path — dispatch into the filesystem's own
    /// `inode->i_fop->read_iter` (e.g. `ext4_file_read_iter`).  We
    /// synth a (kiocb, iov_iter, kvec) triple pointing at a kernel
    /// staging buffer; the .ko's read path drives our `filemap_read`
    /// → `read_cache_folio` → `a_ops->read_folio` chain to populate
    /// the page-cache and copy bytes into the kvec.  After return
    /// we copy the staging buffer into the caller's UserBufferMut.
    fn read_via_read_iter(
        &self, offset: usize, want: usize,
        buf: UserBufferMut<'_>,
    ) -> VfsResult<usize> {
        // Read i_fop and i_fop->read_iter.
        let i_fop: usize = self.read_inode_field(fl::INODE_I_FOP_OFF);
        if i_fop == 0 {
            log::warn!("kabi: KabiFile({}).read: i_fop is null", self.name);
            return Err(Error::new(Errno::EIO));
        }
        let read_iter_fn: usize = unsafe {
            *((i_fop as *const u8).add(fl::FOPS_READ_ITER_OFF)
                as *const usize)
        };
        if read_iter_fn == 0 {
            log::warn!(
                "kabi: KabiFile({}).read: i_fop->read_iter is null",
                self.name,
            );
            return Err(Error::new(Errno::EIO));
        }

        // Allocate kernel staging buffer.
        let staging = super::alloc::kmalloc(want, 0) as *mut u8;
        if staging.is_null() {
            return Err(Error::new(Errno::ENOMEM));
        }

        // Synth struct file with f_inode + f_mapping.
        let file = super::alloc::kzalloc(fl::FILE_SIZE, 0) as *mut u8;
        if file.is_null() {
            super::alloc::kfree(staging as *mut c_void);
            return Err(Error::new(Errno::ENOMEM));
        }
        let i_mapping: usize = self.read_inode_field(fl::INODE_I_MAPPING_OFF);
        unsafe {
            *(file.add(fl::FILE_F_MAPPING_OFF) as *mut usize) = i_mapping;
            *(file.add(fl::FILE_F_INODE_OFF) as *mut usize) = self.inode;
        }

        // Synth kvec (single segment).
        let kvec = super::alloc::kzalloc(fl::KVEC_SIZE, 0) as *mut u8;
        if kvec.is_null() {
            super::alloc::kfree(file as *mut c_void);
            super::alloc::kfree(staging as *mut c_void);
            return Err(Error::new(Errno::ENOMEM));
        }
        unsafe {
            *(kvec.add(fl::KVEC_IOV_BASE_OFF) as *mut *mut u8) = staging;
            *(kvec.add(fl::KVEC_IOV_LEN_OFF) as *mut usize) = want;
        }

        // Synth iov_iter.
        let iter = super::alloc::kzalloc(fl::IOV_ITER_SIZE, 0) as *mut u8;
        if iter.is_null() {
            super::alloc::kfree(kvec as *mut c_void);
            super::alloc::kfree(file as *mut c_void);
            super::alloc::kfree(staging as *mut c_void);
            return Err(Error::new(Errno::ENOMEM));
        }
        unsafe {
            *(iter.add(fl::IOV_ITER_TYPE_OFF)) = fl::ITER_KVEC;
            *(iter.add(fl::IOV_ITER_NOFAULT_OFF)) = 0;
            *(iter.add(fl::IOV_ITER_DATA_SOURCE_OFF)) = fl::ITER_DEST;
            *(iter.add(fl::IOV_ITER_IOV_OFFSET_OFF) as *mut usize) = 0;
            *(iter.add(fl::IOV_ITER_KVEC_OFF) as *mut *mut u8) = kvec;
            *(iter.add(fl::IOV_ITER_COUNT_OFF) as *mut usize) = want;
            *(iter.add(fl::IOV_ITER_NR_SEGS_OFF) as *mut usize) = 1;
        }

        // Synth kiocb.
        let kiocb = super::alloc::kzalloc(fl::KIOCB_SIZE, 0) as *mut u8;
        if kiocb.is_null() {
            super::alloc::kfree(iter as *mut c_void);
            super::alloc::kfree(kvec as *mut c_void);
            super::alloc::kfree(file as *mut c_void);
            super::alloc::kfree(staging as *mut c_void);
            return Err(Error::new(Errno::ENOMEM));
        }
        unsafe {
            *(kiocb.add(fl::KIOCB_KI_FILP_OFF) as *mut *mut u8) = file;
            *(kiocb.add(fl::KIOCB_KI_POS_OFF) as *mut i64) = offset as i64;
            // ki_flags = 0 (buffered, sync, no NOWAIT).
        }

        log::info!(
            "kabi: KabiFile({}).read: dispatching read_iter \
             (want={} offset={})",
            self.name, want, offset,
        );
        let rc = super::loader::call_with_scs_2(
            read_iter_fn as *const (),
            kiocb as usize,
            iter as usize,
        ) as isize;
        log::info!(
            "kabi: KabiFile({}).read: read_iter returned {}",
            self.name, rc,
        );

        // Free temporaries (staging is freed last after copy).
        super::alloc::kfree(kiocb as *mut c_void);
        super::alloc::kfree(iter as *mut c_void);
        super::alloc::kfree(kvec as *mut c_void);
        super::alloc::kfree(file as *mut c_void);

        if rc < 0 {
            super::alloc::kfree(staging as *mut c_void);
            return Err(Error::new(Errno::EIO));
        }
        let copied = (rc as usize).min(want);
        let mut writer = UserBufWriter::from(buf);
        let src = unsafe { core::slice::from_raw_parts(staging, copied) };
        let res = writer.write_bytes(src);
        super::alloc::kfree(staging as *mut c_void);
        res.map_err(|_| Error::new(Errno::EIO))?;
        Ok(copied)
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
        &self, offset: usize, buf: UserBufferMut<'_>,
        _options: &OpenOptions,
    ) -> VfsResult<usize> {
        let i_size: i64 = self.read_inode_field(fl::INODE_I_SIZE_OFF);
        let i_size = i_size.max(0) as usize;
        if offset >= i_size {
            return Ok(0);
        }
        let want = buf.len().min(i_size - offset);
        if want == 0 {
            return Ok(0);
        }

        // Fast path for filesystems with KabiInodeMeta registered
        // (erofs Phase 6: layout decoder + raw initramfs read).
        if let Some(meta) = super::inode_meta::lookup_meta(self.inode) {
            return self.read_via_inode_meta(offset, want, buf, meta);
        }

        // Phase 13: dispatch into the fs's own `read_iter` so ext4
        // (and any future fs) handles its on-disk layout natively.
        // Builds a synth (kiocb, iov_iter, kvec) triple, invokes
        // `inode->i_fop->read_iter` via SCS, then copies the kernel
        // staging buffer into the caller's UserBufferMut.
        self.read_via_read_iter(offset, want, buf)
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
