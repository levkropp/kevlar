# Filesystems

## VFS Layer

Kevlar's VFS (`libs/kevlar_vfs/`) provides a uniform interface over all filesystems.
The crate is `#![forbid(unsafe_code)]` and defines the Ring 2 service boundary.

### INode

```rust
pub enum INode {
    FileLike(Arc<dyn FileLike>),
    Directory(Arc<dyn Directory>),
    Symlink(Arc<dyn Symlink>),
}
```

All filesystem operations go through these traits. The kernel holds `INode` values;
it never calls filesystem-specific code directly.

### FileLike

The primary I/O trait. Every file descriptor ultimately points to an `Arc<dyn FileLike>`:

```rust
pub trait FileLike: Debug + Send + Sync + Downcastable {
    fn read(&self, offset: usize, buf: UserBufferMut, options: &OpenOptions) -> Result<usize>;
    fn write(&self, offset: usize, buf: UserBuffer, options: &OpenOptions) -> Result<usize>;
    fn stat(&self) -> Result<Stat>;
    fn poll(&self) -> Result<PollStatus>;
    fn ioctl(&self, cmd: usize, arg: usize) -> Result<isize>;
    fn truncate(&self, length: usize) -> Result<()>;
    fn chmod(&self, mode: FileMode) -> Result<()>;
    fn fsync(&self) -> Result<()>;
    fn is_content_immutable(&self) -> bool;  // Page cache hint
    // Socket methods: bind, listen, accept, connect, sendto, recvfrom, ...
}
```

Regular files, pipes, sockets, TTY devices, `/dev/null`, eventfd, epoll, signalfd,
timerfd, and inotify instances all implement `FileLike`.

### Directory

```rust
pub trait Directory: Debug + Send + Sync + Downcastable {
    fn lookup(&self, name: &str) -> Result<INode>;
    fn create_file(&self, name: &str, mode: FileMode) -> Result<INode>;
    fn create_dir(&self, name: &str, mode: FileMode) -> Result<INode>;
    fn create_symlink(&self, name: &str, target: &str) -> Result<INode>;
    fn link(&self, name: &str, link_to: &INode) -> Result<()>;
    fn unlink(&self, name: &str) -> Result<()>;
    fn rmdir(&self, name: &str) -> Result<()>;
    fn rename(&self, old_name: &str, new_dir: &Arc<dyn Directory>, new_name: &str) -> Result<()>;
    fn readdir(&self, index: usize) -> Result<Option<DirEntry>>;
    fn stat(&self) -> Result<Stat>;
    fn inode_no(&self) -> Result<INodeNo>;
    fn dev_id(&self) -> usize;
    fn mount_key(&self) -> Result<MountKey>;
    // ...
}
```

### MountKey

Each filesystem allocates a globally unique `dev_id` via an atomic counter. A
`MountKey` is `(dev_id, inode_no)` — this prevents mount point collisions when
different filesystems reuse inode numbers:

```rust
pub struct MountKey {
    pub dev_id: usize,
    pub inode_no: INodeNo,
}
```

### PathComponent and Path Resolution

`PathComponent` is a node in the path tree:

```rust
pub struct PathComponent {
    pub parent_dir: Option<Arc<PathComponent>>,
    pub name: String,
    pub inode: INode,
}
```

Path resolution walks the tree from the process's root or CWD. Two paths:

- **Fast path**: Direct directory tree walk when the path has no `..` and no
  intermediate symlinks. Avoids heap allocation.
- **Full path**: Builds a `PathComponent` chain, follows symlinks (up to 8 hops to
  prevent `ELOOP`), and resolves `..` by walking parent pointers.

Mount points are resolved at each component by looking up the directory's `MountKey`
in the mount table.

### OpenedFileTable

A per-process table mapping file descriptors (integers) to `Arc<OpenedFile>`:

```rust
pub struct OpenedFile {
    path: Arc<PathComponent>,
    pos: AtomicCell<usize>,              // File position (lock-free)
    options: AtomicRefCell<OpenOptions>,  // O_APPEND, O_NONBLOCK, etc.
}

pub struct OpenedFileTable {
    files: Vec<Option<LocalOpenedFile>>,  // Indexed by fd (max 1024)
}

struct LocalOpenedFile {
    opened_file: Arc<OpenedFile>,
    close_on_exec: bool,
}
```

`Arc<OpenedFile>` allows sharing across `fork()`. FD allocation always returns the
lowest available descriptor (POSIX requirement). `O_CLOEXEC` is tracked per-fd and
respected on `execve`.

## Filesystem Implementations

### initramfs

A read-only CPIO newc archive embedded in the kernel image. Parsed at boot by
`services/kevlar_initramfs`. All files are backed by `&'static [u8]` slices — reads
are zero-copy into the page cache. The crate is `#![forbid(unsafe_code)]`.

Files report `is_content_immutable() == true`, allowing the page cache to share
physical pages directly (no copy needed for read-only mappings).

### tmpfs

An in-memory read-write filesystem (`services/kevlar_tmpfs`,
`#![forbid(unsafe_code)]`). Supports regular files, directories, symlinks, hard links,
and all standard POSIX operations.

```rust
pub struct Dir {
    inode_no: INodeNo,
    dev_id: usize,
    inner: SpinLock<DirInner>,
}

struct DirInner {
    files: HashMap<String, TmpFsINode>,
}

pub struct File {
    inode_no: INodeNo,
    data: SpinLock<Vec<u8>>,
}
```

File data is stored in `Vec<u8>`. Directory entries are stored in a `HashMap`. All
locks use `lock_no_irq()` since tmpfs is never accessed from interrupt context.

Used for `/`, `/tmp`, and all runtime-created files.

### ext2 (read-write)

A clean-room ext2/ext3/ext4 implementation on VirtIO block
(`services/kevlar_ext2`, `#![forbid(unsafe_code)]`).

**Supported features:**
- Block pointer traversal (direct, single/double indirect)
- ext4 extent tree reading (B+ tree navigation up to 5 levels)
- 64-bit block addresses (ext4 `INCOMPAT_64BIT`)
- Block and inode allocation/deallocation with bitmap management
- File creation, deletion, truncation, and rename
- Directory creation and removal
- Superblock and group descriptor writeback

```rust
pub struct Ext2Fs {
    inner: Arc<Ext2Inner>,
}

struct Ext2Inner {
    device: Arc<dyn BlockDevice>,
    superblock: Ext2Superblock,
    block_size: usize,
    is_64bit: bool,
    state: SpinLock<Ext2MutableState>,  // Group descriptors, free counts
    dev_id: usize,
}
```

**Block resolution** follows the classic ext2 scheme for block pointers:

```rust
fn resolve_block_ptr(&self, inode: &Ext2Inode, block_index: usize) -> Result<u32> {
    if block_index < 12 { return Ok(inode.block[block_index]); }         // Direct
    let index = block_index - 12;
    if index < ptrs_per_block { /* single indirect via inode.block[12] */ }
    if index < ptrs_per_block² { /* double indirect via inode.block[13] */ }
    Err(EFBIG)  // Triple indirect not supported
}
```

For ext4 inodes with the `EXTENTS` flag, extent tree traversal is used instead:

```rust
fn resolve_extent(&self, inode: &Ext2Inode, logical_block: usize) -> Result<u64> {
    // Parse extent header from inode.block[0..15]
    // If depth == 0: scan leaf extents for matching block range
    // If depth > 0: binary search internal indices, recurse into child node
}
```

**Limitations:** Extent tree *creation* is not implemented (new files use block
pointers). Journal recovery is not performed. Checksums are parsed but not verified.

### procfs

Mounted at `/proc`. A hybrid implementation: static system-wide files are stored in
a tmpfs backing store, while per-process directories (`/proc/[pid]/`) are generated
dynamically on lookup.

```rust
impl Directory for ProcRootDir {
    fn lookup(&self, name: &str) -> Result<INode> {
        if name == "self" { return Ok(INode::Symlink(ProcSelfSymlink)); }
        if let Ok(pid) = name.parse::<i32>() {
            return Ok(INode::Directory(ProcPidDir::new(pid)));
        }
        self.static_dir.lookup(name)  // Fall through to tmpfs
    }
}
```

**System-wide files:**

| Path | Content |
|---|---|
| `/proc/mounts` | Mount table |
| `/proc/filesystems` | Registered filesystem types |
| `/proc/cmdline` | Kernel command line |
| `/proc/stat` | CPU time and process counts |
| `/proc/meminfo` | Memory statistics |
| `/proc/version` | Kernel version string |
| `/proc/cpuinfo` | CPU count and model |
| `/proc/uptime` | System uptime in seconds |
| `/proc/loadavg` | Load averages (stub) |
| `/proc/cgroups` | Cgroup controller list |
| `/proc/sys/kernel/hostname` | Hostname (writable) |
| `/proc/sys/kernel/osrelease` | `"4.0.0"` |
| `/proc/sys/kernel/ostype` | `"Linux"` |
| `/proc/net/{dev,tcp,udp,...}` | Network statistics (stubs) |

**Per-process files (`/proc/[pid]/`):**

| Path | Content |
|---|---|
| `stat` | PID, comm, state, PPID, CPU time, threads |
| `status` | Name, state, PID, UID/GID, VM size, signal masks |
| `maps` | Virtual memory areas (one VMA per line) |
| `fd/` | Open file descriptors as symlinks |
| `cmdline` | Process argv, NUL-separated |
| `comm` | Executable name |
| `cgroup` | Cgroup membership |
| `mountinfo` | Per-process mount table |
| `environ` | Environment variables |
| `exe` | Symlink to executable path |

### sysfs

Mounted at `/sys`. Provides device attributes populated at boot:

```rust
// /sys/class/{tty,mem,misc,net}/   — character device classes
// /sys/block/vda/                  — block device (VirtIO)
// Each device has "dev" and "uevent" attribute files
```

Device nodes report their `major:minor` numbers. The device table is currently
hard-coded for known VirtIO and serial devices.

### devfs

Mounted at `/dev`. Provides device nodes backed by kernel-internal implementations:

| Node | Description |
|---|---|
| `/dev/null` | Discards all writes; reads return EOF |
| `/dev/zero` | Reads return zero bytes |
| `/dev/full` | Writes return `ENOSPC` |
| `/dev/urandom` | Reads return random bytes (RDRAND/RDSEED) |
| `/dev/kmsg` | Writes are logged to kernel serial output |
| `/dev/console` | Serial console TTY |
| `/dev/tty` | Controlling terminal |
| `/dev/ttyS0` | Serial port 0 |
| `/dev/ptmx` | Pseudo-terminal master multiplexer |
| `/dev/pts/N` | Pseudo-terminal slave devices |
| `/dev/shm/` | POSIX shared memory directory |

Device node files implement `FileLike::open()` to redirect to the real device driver
via a `(major, minor)` lookup table.

## Mount Namespace

`mount(2)` adds entries to the mount table. Each entry maps a `MountKey` to a
filesystem root. During path resolution, the mount table is checked at each component
to detect mount points.

Boot-time mounts:

| Mount point | Filesystem |
|---|---|
| `/` | initramfs |
| `/proc` | procfs |
| `/dev` | devfs |
| `/tmp` | tmpfs |
| `/sys` | sysfs |
| `/sys/fs/cgroup` | cgroupfs |

`pivot_root` is supported for container-style filesystem isolation.

## inotify

The inotify subsystem (`kernel/fs/inotify.rs`) watches paths for filesystem events.
A global registry maps watched paths to `InotifyInstance` handles:

```rust
pub fn notify(dir_path: &str, name: &str, mask: u32) {
    for instance in REGISTRY.lock().iter() {
        instance.match_and_queue(dir_path, name, mask, 0);
    }
    POLL_WAIT_QUEUE.wake_all();
}
```

Supported events: `IN_CREATE`, `IN_DELETE`, `IN_MODIFY`, `IN_OPEN`, `IN_CLOSE_WRITE`,
`IN_CLOSE_NOWRITE`, `IN_MOVED_FROM`, `IN_MOVED_TO`, `IN_ACCESS`, `IN_ATTRIB`,
`IN_DELETE_SELF`, `IN_MOVE_SELF`.

Rename events use a shared atomic cookie counter for pairing `IN_MOVED_FROM` /
`IN_MOVED_TO`. Events are queued in a ring buffer and readable via `read(2)`. `poll`
and `epoll` work on inotify file descriptors.

## File Metadata

Supported metadata operations: `stat`, `fstat`, `lstat`, `newfstatat`, `statx`,
`statfs`, `fstatfs`, `utimensat`, `fallocate`, `fadvise64`.

Advisory file locking (`flock`) is implemented. Mandatory locking is not.
