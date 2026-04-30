// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Linux struct field offsets used by the kABI fs adapter.
//!
//! All offsets pinned to **Linux 7.0.0-14** (the version of erofs.ko
//! we load via Ubuntu's `linux-modules-7.0.0-14-generic.deb`),
//! sourced from `build/linux-src.vanilla-v7.0/include/linux/`.
//!
//! These constants are documented one-by-one against the matching
//! header file + line.  Do **not** change blindly — each offset
//! determines whether dereferencing a synthesised struct lands on
//! the right field or in random memory.  When in doubt, log the
//! field's value at runtime and compare against an expected value
//! (e.g. `name` pointers point at known strings; `i_mode` should
//! decode to a sensible umode_t).

// ── struct file (include/linux/fs.h:1259) ───────────────────────
//
//   +0   spinlock_t f_lock                            (4 bytes)
//   +4   fmode_t f_mode                               (4 bytes)
//   +8   const struct file_operations *f_op           (8 bytes)
//   +16  struct address_space *f_mapping              (8 bytes)
//   +24  void *private_data                           (8 bytes)
//   +32  struct inode *f_inode                        (8 bytes)
//   +40  unsigned int f_flags                         (4 bytes)
//   +44  unsigned int f_iocb_flags                    (4 bytes)
//   +48  const struct cred *f_cred                    (8 bytes)
//   +56  struct fown_struct *f_owner                  (8 bytes)
//   +64  union { struct path f_path; ... }            (16 bytes)
//   ...  rest unused by erofs's mount-time S_ISREG check
//
// We allocate 256 bytes; erofs only reads f_mapping + f_inode at
// mount time.

pub const FILE_F_MAPPING_OFF: usize = 16;
pub const FILE_F_INODE_OFF: usize = 32;
pub const FILE_SIZE: usize = 256;

// ── struct inode (include/linux/fs.h:766) ───────────────────────
//
//   +0   umode_t i_mode                               (2 bytes)
//   +2   unsigned short i_opflags                     (2 bytes)
//   +4   unsigned int i_flags                         (4 bytes)
//   +8   struct posix_acl *i_acl       [POSIX_ACL]    (8 bytes)
//   +16  struct posix_acl *i_default_acl              (8 bytes)
//   +24  kuid_t i_uid                                 (4 bytes)
//   +28  kgid_t i_gid                                 (4 bytes)
//   +32  const struct inode_operations *i_op          (8 bytes)
//   +40  struct super_block *i_sb                     (8 bytes)
//   +48  struct address_space *i_mapping              (8 bytes)
//   +56  void *i_security                             (8 bytes)
//   +64  unsigned long i_ino                          (8 bytes)
//   +72  union { i_nlink, __i_nlink }                 (4 bytes + 4 pad)
//   +80  loff_t i_size                                (8 bytes)
//   ...  many more fields; we allocate 1024 bytes to be safe.

pub const INODE_I_MODE_OFF: usize = 0;
pub const INODE_I_OP_OFF: usize = 32;
pub const INODE_I_SB_OFF: usize = 40;
pub const INODE_I_MAPPING_OFF: usize = 48;
pub const INODE_I_INO_OFF: usize = 64;
pub const INODE_I_SIZE_OFF: usize = 80;
/// `i_blkbits` (u8) — log2 of inode blocksize.  Linux's
/// `inode_init_always` sets this to `sb->s_blocksize_bits` at
/// inode-allocation time.  Erofs's `find_target_block_classic`
/// reads `dir->[+134]` to compute `iblks = round_up(i_size, blksz)
/// >> blkbits`; if blkbits=0 the binary search probes far past
/// EOF and returns -EFSCORRUPTED.  Verified via erofs.ko disasm
/// at offset 0x8164 (`ldrb w23, [x1, #134]`).
pub const INODE_I_BLKBITS_OFF: usize = 134;
pub const INODE_SIZE: usize = 1024;

// S_IFREG and access-mode constants (include/uapi/linux/stat.h).
pub const S_IFREG: u16 = 0o100000;
pub const S_IFDIR: u16 = 0o040000;

// ── struct address_space (include/linux/fs.h:470) ───────────────
//
// Layout reasoning (Ubuntu config: RWSEM_SPIN_ON_OWNER=y, no debug,
// no READ_ONLY_THP_FOR_FS):
//
//   +0   struct inode *host                           (8 bytes)
//   +8   struct xarray i_pages                        (16 bytes)
//        = { spinlock_t lock(4), gfp_t gfp(4), void *head(8) }
//   +24  struct rw_semaphore invalidate_lock          (40 bytes)
//        = { atomic_long_t count(8), atomic_long_t owner(8),
//            optimistic_spin_queue osq(4), raw_spinlock_t lock(4),
//            list_head wait_list(16) }
//   +64  gfp_t gfp_mask                               (4 bytes)
//   +68  atomic_t i_mmap_writable                     (4 bytes)
//   +72  struct rb_root_cached i_mmap                 (16 bytes)
//   +88  unsigned long nrpages                        (8 bytes)
//   +96  pgoff_t writeback_index                      (8 bytes)
//   +104 const struct address_space_operations *a_ops (8 bytes)
//   +112 unsigned long flags                          (8 bytes)
//   ...  rest unused at mount time.

pub const AS_HOST_OFF: usize = 0;
pub const AS_A_OPS_OFF: usize = 104;
pub const AS_SIZE: usize = 256;

// ── struct address_space_operations (include/linux/fs.h:403) ────
//
//   +0   int (*read_folio)(struct file *, struct folio *)
//   +8   int (*writepages)(...)
//   +16  bool (*dirty_folio)(...)
//   +24  void (*readahead)(...)
//   +32  int (*write_begin)(...)
//   ...  19 fn pointers total, 152 bytes

pub const AOPS_READ_FOLIO_OFF: usize = 0;
pub const AOPS_SIZE: usize = 152;

// ── struct file_operations (include/linux/fs.h) ─────────────────
//
// Linux 7.0 layout:
//   +0   struct module *owner
//   +8   fop_flags_t fop_flags (4 bytes + 4 pad)
//   +16  loff_t (*llseek)(...)
//   +24  ssize_t (*read)(...)
//   +32  ssize_t (*write)(...)
//   +40  ssize_t (*read_iter)(struct kiocb *, struct iov_iter *)
//   +48  ssize_t (*write_iter)(...)
//   +56  int (*iopoll)(...)
//   +64  int (*iterate_shared)(...)
//   ...
pub const FOPS_READ_ITER_OFF: usize = 40;
pub const FOPS_ITERATE_SHARED_OFF: usize = 64;
// Continuing the file_operations layout:
//   +72  poll
//   +80  unlocked_ioctl
//   +88  compat_ioctl
//   +96  mmap
//   +104 open(struct inode *, struct file *)
//   +112 flush
//   +120 release(struct inode *, struct file *)
pub const FOPS_OPEN_OFF: usize = 104;
pub const FOPS_RELEASE_OFF: usize = 120;

// ── struct kiocb (include/linux/fs.h:381) ───────────────────────
//
//   +0   struct file *ki_filp
//   +8   loff_t ki_pos
//   +16  void (*ki_complete)(struct kiocb *, long)
//   +24  void *private
//   +32  int ki_flags
//   +36  u16 ki_ioprio
//   +38  u8 ki_write_stream
//   +40  struct wait_page_queue *ki_waitq
// Total: 48 bytes.
pub const KIOCB_KI_FILP_OFF: usize = 0;
pub const KIOCB_KI_POS_OFF: usize = 8;
pub const KIOCB_KI_FLAGS_OFF: usize = 32;
pub const KIOCB_SIZE: usize = 48;

// ── struct iov_iter (include/linux/uio.h:43) ────────────────────
//
//   +0   u8 iter_type
//   +1   bool nofault
//   +2   bool data_source
//   +3..+7 pad (alignment to 8)
//   +8   size_t iov_offset
//   +16  union { iov, kvec, bvec, ubuf, ... }: 8-byte ptr
//   +24  size_t count   (or 16-byte iovec for ITER_UBUF)
//   +32  union { unsigned long nr_segs, ... }
// Total: 40 bytes.
pub const IOV_ITER_TYPE_OFF: usize = 0;
pub const IOV_ITER_NOFAULT_OFF: usize = 1;
pub const IOV_ITER_DATA_SOURCE_OFF: usize = 2;
pub const IOV_ITER_IOV_OFFSET_OFF: usize = 8;
pub const IOV_ITER_KVEC_OFF: usize = 16;
pub const IOV_ITER_COUNT_OFF: usize = 24;
pub const IOV_ITER_NR_SEGS_OFF: usize = 32;
pub const IOV_ITER_SIZE: usize = 40;

// `enum iter_type` from include/linux/uio.h:23
//   ITER_UBUF=0, ITER_IOVEC=1, ITER_BVEC=2, ITER_KVEC=3, ...
pub const ITER_KVEC: u8 = 3;
// `data_source`: ITER_DEST=0 (read), ITER_SOURCE=1 (write).
pub const ITER_DEST: u8 = 0;

// ── struct kvec (include/linux/uio.h:18) ────────────────────────
//
//   +0   void *iov_base
//   +8   size_t iov_len
// Total: 16 bytes.
pub const KVEC_IOV_BASE_OFF: usize = 0;
pub const KVEC_IOV_LEN_OFF: usize = 8;
pub const KVEC_SIZE: usize = 16;

// `inode->i_fop` (file_operations *) — verified via Phase 4 erofs.ko
// disasm (`erofs_fill_inode` storing at offset 0x160 from inode).
pub const INODE_I_FOP_OFF: usize = 352;

// ── struct super_block (include/linux/fs.h, larger) ─────────────
//
// Allocate 4096 bytes — super_block is ~1700 bytes in modern Linux
// with all CONFIGs.  We zero-fill and let erofs_fc_fill_super
// populate fields it cares about.  Field offsets we need to read:
//
//   s_blocksize, s_blocksize_bits — set before fill_super
//   s_root         — written by fill_super; read by KabiFileSystem
//   s_fs_info      — pre-populated by erofs's init_fs_context
//   s_op           — set by fill_super
//   s_dev          — for inode_no()/dev_id() in KabiDirectory
//
// Offsets verified iteratively as erofs reads them.  Initial guess
// based on header layout walk:

// Layout verified against erofs.ko disasm at offset 0x49c0:
//   str x0, [x19, #24]   ; sb->s_blocksize at +24 (8 bytes)
//   strb w0, [x19, #20]  ; sb->s_blocksize_bits at +20 (1 byte)
pub const SB_S_LIST_OFF: usize = 0;        // struct list_head s_list (16)
pub const SB_S_DEV_OFF: usize = 16;        // dev_t (4)
pub const SB_S_BLOCKSIZE_BITS_OFF: usize = 20;  // u8
pub const SB_S_BLOCKSIZE_OFF: usize = 24;       // u64 (8 bytes, padding aligns)
pub const SB_S_MAXBYTES_OFF: usize = 32;        // loff_t
pub const SB_S_TYPE_OFF: usize = 40;            // file_system_type *
pub const SB_S_OP_OFF: usize = 48;              // super_operations *
// sb->s_flags at +80: after dq_op +56, s_qcop +64, s_export_op +72.
// Verified by counting from struct super_block in
// include/linux/fs/super_types.h (Linux 7.0).  s_flags is unsigned long.
pub const SB_S_FLAGS_OFF: usize = 80;
// `sb->s_root` verified via erofs.ko `fc_fill_super` disasm at 0x4940:
//   str x0, [x19, #104]   ; sb[+104] = result of d_make_root
// Was guessed at 256; real offset is 104.
pub const SB_S_ROOT_OFF: usize = 104;
// `sb->s_fs_info` verified via erofs.ko `erofs_read_superblock` disasm:
//   43ec: ldr x20, [x22, #912]   ; x22 = sb arg, x20 = sb->s_fs_info
// Subsequent reads at offsets 912/256-aligned in fc_fill_super and
// erofs_iget5_set match.  Was guessed at 320; that misalignment caused
// sbi reads to come back zero, sbi writes to land at user-VA 0+offset,
// and crc32c to be called with len=blocksize-8 from blkszbits=0.
pub const SB_S_FS_INFO_OFF: usize = 912;
/// `sb->s_bdev` — verified via erofs.ko's fc_fill_super disasm at
/// 0x47d4 (`ldr x0, [x19, #232]`) which loads sb->s_bdev to check
/// for non-null before taking the bdev branch.
pub const SB_S_BDEV_OFF: usize = 232;
pub const SB_SIZE: usize = 4096;

// ── struct buffer_head (include/linux/buffer_head.h) ────────────
//
// Linux 7.0 layout (pre-CONFIG_DEBUG_KERNEL, no extra debug fields):
//   +0    unsigned long      b_state         (8)
//   +8    struct buffer_head *b_this_page    (8)
//   +16   struct page *      b_page          (8)  (union with b_folio)
//   +24   sector_t           b_blocknr       (8)
//   +32   size_t             b_size          (8)
//   +40   char *             b_data          (8)
//   +48   struct block_device *b_bdev        (8)
//   +56   bh_end_io_t *      b_end_io        (8)
//   +64   void *             b_private       (8)
//   ...   list_head + assoc_map + b_count + b_uptodate_lock
//
// Allocate 128 bytes — covers all fields ext4/jbd2 actually read.
pub const BH_B_STATE_OFF:    usize = 0;
pub const BH_B_BLOCKNR_OFF:  usize = 24;
pub const BH_B_SIZE_OFF:     usize = 32;
pub const BH_B_DATA_OFF:     usize = 40;
pub const BH_B_BDEV_OFF:     usize = 48;
pub const BH_SIZE: usize = 128;

/// Bit 0 of `b_state` — set by every successful block read.  Filesystems
/// check `buffer_uptodate(bh)` (= `test_bit(BH_Uptodate, &bh->b_state)`)
/// before reading bh->b_data.
pub const BH_UPTODATE: u64 = 1 << 0;

/// Linux 7.0 `enum bh_state_bits` (include/linux/buffer_head.h):
///   BH_Uptodate=0, BH_Dirty=1, BH_Lock=2, BH_Req=3, ...
/// Real `__lock_buffer` / `unlock_buffer` set/clear bit 2 atomically;
/// ext4's bread chain relies on the lock state to track "I/O in flight".
pub const BH_DIRTY:  u64 = 1 << 1;
pub const BH_LOCK:   u64 = 1 << 2;
pub const BH_REQ:    u64 = 1 << 3;

// ── struct dentry (include/linux/dcache.h:92) ───────────────────
//
// Phase 5 v4 disasm verification (erofs_lookup at .ko offset
// 0x877c reads `ldr w1, [x19, #36]` for `d_name.len`, so qstr
// starts at +32 not +40):
//
//   +0   unsigned int d_flags                         (4 bytes)
//   +4   seqcount_spinlock_t d_seq                    (4 bytes — no LOCKDEP)
//   +8   struct hlist_bl_node d_hash                  (16 bytes)
//   +24  struct dentry *d_parent                      (8 bytes)
//   +32  union { qstr d_name; ... }                   (16 bytes;
//        qstr = { union { hash u32 + len u32; hash_len u64 }; name *u8 })
//        - +32: hash (u32)
//        - +36: len (u32)              ← erofs reads this
//        - +40: name (*u8)
//   +48  struct inode *d_inode                        (per source)
//   +56  union shortname_store d_shortname            (per source)
//   ...
//
// HOWEVER, our kABI d_make_root + d_splice_alias write `inode`
// at +56 and our adapter reads it back at +56.  Because Linux's
// erofs_lookup DOESN'T read dentry->d_inode directly (it goes
// through d_splice_alias which is our shim), our +56 convention
// is internally consistent and the actual on-disk vmlinux layout
// of `d_inode` doesn't matter for our codepath.  Same logic
// applies to `d_parent` and `d_sb` — we never read them via
// erofs's compiled code, so their offsets are our private
// convention.

pub const DENTRY_D_FLAGS_OFF: usize = 0;
/// Erofs reads `dentry->d_name.len` at +36 (verified disasm).
/// qstr starts at +32: hash u32, len u32, name *u8.
pub const DENTRY_D_NAME_OFF: usize = 32;
/// Our private convention for d_inode storage.  d_make_root and
/// d_splice_alias write here; KabiFileSystem.root_dir + lookup
/// read from here.  Erofs never reads d_inode directly.
pub const DENTRY_D_INODE_OFF: usize = 56;
/// Our private convention for d_parent storage.  Not read by erofs.
pub const DENTRY_D_PARENT_OFF: usize = 64;
/// Our private convention for d_sb storage.  Not read by erofs.
pub const DENTRY_D_SB_OFF: usize = 72;
pub const DENTRY_SIZE: usize = 256;

// ── struct fs_context (include/linux/fs_context.h:90) ───────────
//
// (Already used in fs_adapter.rs; centralise here for reference.)

// fs_context layout (Ubuntu 7.0 arm64 with no-debug, RWSEM_SPIN_ON_OWNER=y):
//   +0   const struct fs_context_operations *ops    (8)
//   +8   struct mutex uapi_mutex                    (32)
//   +40  struct file_system_type *fs_type           (8)
//   +48  void *fs_private                           (8)
//   +56  void *sget_key                             (8)
//   +64  struct dentry *root                        (8)
//   +72  struct user_namespace *user_ns             (8)
//   +80  struct net *net_ns                         (8)
//   +88  const struct cred *cred                    (8)
//   +96  struct p_log log                           (16)
//   +112 const char *source                         (8)
//   +120 void *security                             (8)
//   +128 void *s_fs_info                            (8)
//   +136 unsigned int sb_flags                      (4)
//   +140 unsigned int sb_flags_mask                 (4)
//   +144 unsigned int s_iflags                      (4)
//   +148 fs_context_purpose:8 + phase:8 + bools     (4 incl. padding)
pub const FC_OPS_OFF: usize = 0;
pub const FC_FS_TYPE_OFF: usize = 40;
pub const FC_FS_PRIVATE_OFF: usize = 48;
pub const FC_ROOT_OFF: usize = 64;          // struct dentry *root
pub const FC_SOURCE_OFF: usize = 112;
pub const FC_S_FS_INFO_OFF: usize = 128;    // fs's per-mount stash (erofs_sb_info)
pub const FC_SB_FLAGS_OFF: usize = 136;
pub const FC_PURPOSE_OFF: usize = 148;

// ── struct fs_context_operations (include/linux/fs_context.h:115) ─

pub const FC_OPS_FREE_OFF: usize = 0;
pub const FC_OPS_DUP_OFF: usize = 8;
pub const FC_OPS_PARSE_PARAM_OFF: usize = 16;
pub const FC_OPS_PARSE_MONOLITHIC_OFF: usize = 24;
pub const FC_OPS_GET_TREE_OFF: usize = 32;
pub const FC_OPS_RECONFIGURE_OFF: usize = 40;

// ── struct file_system_type (include/linux/fs.h:2271) ───────────

pub const FST_NAME_OFF: usize = 0;
pub const FST_FS_FLAGS_OFF: usize = 8;
pub const FST_INIT_FS_CONTEXT_OFF: usize = 16;
pub const FST_PARAMETERS_OFF: usize = 24;
pub const FST_KILL_SB_OFF: usize = 32;
pub const FST_OWNER_OFF: usize = 40;
pub const FST_NEXT_OFF: usize = 48;

// ── SB flags (include/linux/fs.h, "Possible states of 'frozen' field") ─
pub const SB_RDONLY: u32 = 1;
pub const FS_CONTEXT_FOR_MOUNT: u8 = 1;

// ── arm64 7.0 VA model (decoded from erofs.ko `kmap_local_page`) ─
//
// Verified by disassembling `erofs_bread+0x138` in
// `linux-modules-7.0.0-14-generic`.  See `kabi::folio_shadow` for the
// implementation that uses these constants.
//
//   PAGE_OFFSET     = 0xffff_0000_0000_0000  (= Kevlar's KERNEL_BASE_ADDR)
//   VMEMMAP_START   = 0xffff_fdff_c000_0000
//   VMEMMAP_END     = -SZ_1G = 0xffff_ffff_c000_0000
//   sizeof(struct page) = 64
//
// Linux's inline `kmap_local_page(folio)`:
//     idx = (folio - VMEMMAP_START) / sizeof(struct page)
//     va  = PAGE_OFFSET + idx * PAGE_SIZE
pub const LINUX_PAGE_OFFSET: u64 = 0xffff_0000_0000_0000;
pub const LINUX_VMEMMAP_START: u64 = 0xffff_fdff_c000_0000;
pub const LINUX_SIZEOF_STRUCT_PAGE: u64 = 64;
