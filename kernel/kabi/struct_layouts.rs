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
// Many fields between op and s_root; pin via runtime probe.
pub const SB_S_ROOT_OFF: usize = 256;      // GUESS
pub const SB_S_FS_INFO_OFF: usize = 320;   // GUESS
pub const SB_SIZE: usize = 4096;

// ── struct dentry (include/linux/dcache.h:92) ───────────────────
//
//   +0   unsigned int d_flags                         (4 bytes)
//   +4   seqcount_spinlock_t d_seq                    (~8 bytes)
//   +16  struct hlist_bl_node d_hash                  (16 bytes)
//   +32  struct dentry *d_parent                      (8 bytes)
//   +40  union { qstr d_name; ... }                   (16 bytes; qstr = u32 hash+len, *name)
//   +56  struct inode *d_inode                        (8 bytes)
//   ...  more fields, ~256 bytes total

pub const DENTRY_D_FLAGS_OFF: usize = 0;
pub const DENTRY_D_PARENT_OFF: usize = 32;
pub const DENTRY_D_NAME_OFF: usize = 40;   // struct qstr
pub const DENTRY_D_INODE_OFF: usize = 56;
pub const DENTRY_D_SB_OFF: usize = 88;     // GUESS — verify Day 4
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
