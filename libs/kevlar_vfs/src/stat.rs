// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::inode::INodeNo;

/// The device file's ID.
#[derive(Debug, Copy, Clone)]
#[repr(transparent)]
pub struct DevId(usize);

impl DevId {
    pub const fn new(v: usize) -> Self { Self(v) }
}

/// The number of hard links.
#[derive(Debug, Copy, Clone)]
#[repr(transparent)]
pub struct NLink(usize);

impl NLink {
    pub const fn new(v: usize) -> Self { Self(v) }
}

/// The file size in bytes.
#[derive(Debug, Copy, Clone)]
#[repr(transparent)]
pub struct FileSize(pub isize);

/// The user ID.
#[derive(Debug, Copy, Clone)]
#[repr(transparent)]
pub struct UId(u32);

impl UId {
    pub const fn new(v: u32) -> Self { Self(v) }
}

/// The Group ID.
#[derive(Debug, Copy, Clone)]
#[repr(transparent)]
pub struct GId(u32);

impl GId {
    pub const fn new(v: u32) -> Self { Self(v) }
}

/// The size in bytes of a block file file system I/O operations.
#[derive(Debug, Copy, Clone)]
#[repr(transparent)]
pub struct BlockSize(isize);

impl BlockSize {
    pub const fn new(v: isize) -> Self { Self(v) }
}

/// The number of blocks.
#[derive(Debug, Copy, Clone)]
#[repr(transparent)]
pub struct BlockCount(isize);

impl BlockCount {
    pub const fn new(v: isize) -> Self { Self(v) }
}

/// The file size in bytes.
#[derive(Debug, Copy, Clone)]
#[repr(transparent)]
pub struct Time(isize);

impl Time {
    pub const fn new(v: isize) -> Self { Self(v) }
}

pub const S_IFMT: u32 = 0o170000;
pub const S_IFCHR: u32 = 0o020000;
pub const S_IFBLK: u32 = 0o060000;
pub const S_IFDIR: u32 = 0o040000;
pub const S_IFREG: u32 = 0o100000;
pub const S_IFLNK: u32 = 0o120000;

pub const O_ACCMODE: u32 = 0o3;

// FIXME: OpenFlags also define these values.
#[allow(unused)]
pub const O_RDONLY: u32 = 0o0;
pub const O_WRONLY: u32 = 0o1;
pub const O_RDWR: u32 = 0o2;

#[derive(Debug, Copy, Clone)]
#[repr(transparent)]
pub struct FileMode(u32);

impl FileMode {
    pub fn new(value: u32) -> FileMode {
        FileMode(value)
    }

    pub fn access_mode(self) -> u32 {
        self.0 & O_ACCMODE
    }

    pub fn is_directory(self) -> bool {
        (self.0 & S_IFMT) == S_IFDIR
    }

    pub fn is_regular_file(self) -> bool {
        (self.0 & S_IFMT) == S_IFREG
    }

    pub fn is_symbolic_link(self) -> bool {
        (self.0 & S_IFMT) == S_IFLNK
    }

    pub fn as_u32(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Copy, Clone)]
#[repr(C, packed)]
pub struct Stat {
    pub dev: DevId,
    pub inode_no: INodeNo,
    pub nlink: NLink,
    pub mode: FileMode,
    pub uid: UId,
    pub gid: GId,
    pub pad0: u32,
    pub rdev: DevId,
    pub size: FileSize,
    pub blksize: BlockSize,
    pub blocks: BlockCount,
    pub atime: Time,
    pub atime_nsec: Time,
    pub mtime: Time,
    pub mtime_nsec: Time,
    pub ctime: Time,
    pub ctime_nsec: Time,
    pub _unused: [isize; 3],
}

impl Stat {
    pub fn zeroed() -> Stat {
        Stat {
            dev: DevId(0),
            inode_no: INodeNo::new(0),
            mode: FileMode(0),
            nlink: NLink(1),
            uid: UId(0),
            gid: GId(0),
            pad0: 0,
            rdev: DevId(0),
            size: FileSize(0),
            blksize: BlockSize(4096),
            blocks: BlockCount(0),
            atime: Time(0),
            atime_nsec: Time(0),
            mtime: Time(0),
            mtime_nsec: Time(0),
            ctime: Time(0),
            ctime_nsec: Time(0),
            _unused: [0; 3],
        }
    }

    /// Convert to the architecture-specific ABI binary layout for userspace.
    ///
    /// ARM64 (asm-generic/stat.h): mode(u32)|nlink(u32) at offset 16, blksize=i32.
    /// x86_64: nlink(u64)|mode(u32) at offset 16, blksize=i64.
    #[cfg(target_arch = "aarch64")]
    pub fn to_abi_bytes(&self) -> [u8; 128] {
        let mut buf = [0u8; 128];
        let b = &mut buf;
        put_u64(b, 0, self.dev.0 as u64);
        put_u64(b, 8, self.inode_no.as_u64());
        put_u32(b, 16, self.mode.0);              // mode BEFORE nlink
        put_u32(b, 20, self.nlink.0 as u32);      // nlink is u32
        put_u32(b, 24, self.uid.0);
        put_u32(b, 28, self.gid.0);
        put_u64(b, 32, self.rdev.0 as u64);
        put_u64(b, 40, 0);                        // __pad1
        put_u64(b, 48, self.size.0 as u64);
        put_u32(b, 56, self.blksize.0 as u32);    // blksize is i32
        put_u32(b, 60, 0);                        // __pad2
        put_u64(b, 64, self.blocks.0 as u64);
        put_u64(b, 72, self.atime.0 as u64);
        put_u64(b, 80, self.atime_nsec.0 as u64);
        put_u64(b, 88, self.mtime.0 as u64);
        put_u64(b, 96, self.mtime_nsec.0 as u64);
        put_u64(b, 104, self.ctime.0 as u64);
        put_u64(b, 112, self.ctime_nsec.0 as u64);
        // __unused4, __unused5 at 120..128 left as zero
        buf
    }

    /// x86_64: serialize to the x86_64 `struct stat` layout (144 bytes).
    #[cfg(target_arch = "x86_64")]
    pub fn to_abi_bytes(&self) -> [u8; 144] {
        let mut buf = [0u8; 144];
        let b = &mut buf;
        put_u64(b, 0, self.dev.0 as u64);        // st_dev
        put_u64(b, 8, self.inode_no.as_u64());    // st_ino
        put_u64(b, 16, self.nlink.0 as u64);      // st_nlink (u64 on x86_64)
        put_u32(b, 24, self.mode.0);              // st_mode
        put_u32(b, 28, self.uid.0);               // st_uid
        put_u32(b, 32, self.gid.0);               // st_gid
        // 36: pad0 (4 bytes, zero)
        put_u64(b, 40, self.rdev.0 as u64);       // st_rdev
        put_u64(b, 48, self.size.0 as u64);       // st_size (i64)
        put_u64(b, 56, self.blksize.0 as u64);    // st_blksize (i64 on x86_64)
        put_u64(b, 64, self.blocks.0 as u64);     // st_blocks (i64)
        put_u64(b, 72, self.atime.0 as u64);      // st_atime
        put_u64(b, 80, self.atime_nsec.0 as u64); // st_atime_nsec
        put_u64(b, 88, self.mtime.0 as u64);      // st_mtime
        put_u64(b, 96, self.mtime_nsec.0 as u64); // st_mtime_nsec
        put_u64(b, 104, self.ctime.0 as u64);     // st_ctime
        put_u64(b, 112, self.ctime_nsec.0 as u64);// st_ctime_nsec
        // 120..144: __unused[3] (24 bytes, zero)
        buf
    }
}

fn put_u64(buf: &mut [u8], off: usize, val: u64) {
    buf[off..off + 8].copy_from_slice(&val.to_ne_bytes());
}

fn put_u32(buf: &mut [u8], off: usize, val: u32) {
    buf[off..off + 4].copy_from_slice(&val.to_ne_bytes());
}
