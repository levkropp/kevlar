# M10 Phase 8: The Mount Key Collision

We added a 7-layer Alpine Linux integration test to validate every layer
of the stack bottom-up: ext2 mount, file I/O, chroot, apk database, DNS,
HTTP, and `apk update`. Layer 1 immediately found a showstopper: `busybox`
didn't exist in the mounted ext2 filesystem. Except it did.

## Symptoms

```
PASS l1_mount_ext2
FAIL l1_busybox_exists (stat errno=2)
  /mnt/bin/ contents:
    [0] ino=0 type=8 'cgroup.procs'
    [1] ino=0 type=8 'cgroup.controllers'
    ...
PASS l1_musl_ld_exists
PASS l1_apk_exists
```

`stat("/mnt/bin/busybox")` returned ENOENT, but `stat("/mnt/sbin/apk")`
and `stat("/mnt/lib/ld-musl-x86_64.so.1")` both succeeded. And when we
listed `/mnt/bin/` with `opendir`, it contained **cgroup pseudo-files**
instead of ext2 directory entries.

The ext2 mount was fine — `readdir("/mnt")` correctly listed all Alpine
directories with their ext2 inode numbers. But specifically `/mnt/bin`
resolved to the cgroup2 filesystem root.

## The mount table design

Kevlar's VFS uses a per-process mount point table: a `HashMap<INodeNo,
MountPoint>`. When mounting a filesystem on a directory, the directory's
inode number becomes the key. During path resolution, after looking up
each directory component, the VFS checks if that directory's inode number
is a mount point and, if so, switches to the mounted filesystem's root.

```rust
pub fn mount(&mut self, dir: Arc<dyn Directory>, fs: Arc<dyn FileSystem>) {
    self.mount_points.insert(dir.stat()?.inode_no, MountPoint { fs });
}

fn lookup_mount_point(&self, dir: &Arc<dyn Directory>) -> Option<&MountPoint> {
    self.mount_points.get(&dir.inode_no()?)
}
```

The assumption: inode numbers are unique. This is true **within** a
filesystem, but not **across** filesystems.

## Tracing the collision

The boot sequence initializes three TmpFs-backed filesystems, all sharing
a single global `alloc_inode_no()` counter:

| Order | Filesystem | `add_dir` calls | Counter range |
|-------|-----------|-----------------|---------------|
| 1 | ProcFs | sys, kernel, random, fs, net, unix, net | 2-8 |
| 2 | DevFs | pts, shm | 9-10 |
| 3 | SysFs | fs, **cgroup**, class, devices, bus, kernel, block | 11-17 |

The sysfs `cgroup` directory — the mount target for cgroup2 — got tmpfs
inode **12**.

Meanwhile, `mke2fs -d /alpine-root` assigns ext2 inodes depth-first
alphabetically. After `lost+found` (inode 11), the first root directory
entry is `bin/` — ext2 inode **12**.

```
$ debugfs -R 'ls -l /' build/alpine-disk.img
     11   40700   lost+found
     12   40755   bin          <-- same inode number!
     95   40755   dev
     96   40755   etc
```

When the VFS resolved `/mnt/bin`:
1. "mnt" → initramfs /mnt (inode 296) → mount crossing to ext2 root
2. "bin" → ext2 lookup returns `/bin/` with inode 12
3. Mount table check: inode 12 → **hit** → cgroup2 filesystem

The ext2 `bin/` directory was being transparently replaced by the cgroup2
filesystem root. Every path through `/mnt/bin` saw cgroup control files
instead of Alpine binaries.

## The fix: composite mount keys

The fix is to include a filesystem identifier in the mount key. Each
filesystem instance gets a unique device ID from a global atomic counter:

```rust
pub fn alloc_dev_id() -> usize {
    static NEXT_DEV_ID: AtomicUsize = AtomicUsize::new(1);
    NEXT_DEV_ID.fetch_add(1, Ordering::Relaxed)
}
```

The mount table key changes from bare `INodeNo` to a composite
`MountKey(dev_id, inode_no)`:

```rust
pub struct MountKey {
    pub dev_id: usize,
    pub inode_no: INodeNo,
}
```

The `Directory` trait gets `dev_id()` and `mount_key()` methods. Each
filesystem propagates its unique dev_id to every directory it creates.
TmpFs, ext2, and initramfs all participate.

Now the sysfs `cgroup` directory has mount key `(3, 12)` and the ext2
`bin/` directory has mount key `(5, 12)` — different dev_ids, no collision.

## Why this was invisible until now

The collision requires:
1. Multiple TmpFs-backed filesystems consuming from the shared inode counter
2. An ext2 filesystem whose inode assignments happen to overlap
3. A mount on one of the overlapping inodes

Before the Alpine disk test, the only ext2 image was the 16MB test disk
with a handful of files. Its inode numbers didn't overlap with the sysfs
counter. The Alpine minirootfs, with 500+ files in a depth-first layout
starting from inode 12, hit the exact range consumed by sysfs during boot.

This is the same class of bug that Unix solved decades ago with device
numbers: inode numbers are only unique within a filesystem, and any global
table indexed by inode must also include the device. Linux uses `(dev_t,
ino_t)` pairs throughout its mount infrastructure for exactly this reason.

## The test harness

The Alpine integration test (`testing/test_alpine.c`) validates 7 layers
with dependency tracking:

| Layer | Tests | Depends on |
|-------|-------|-----------|
| 1. Foundation | ext2 mount, file existence, stat | — |
| 2. ext2 Write | create, mkdir, symlink, rename, large file | Layer 1 |
| 3. chroot + Dynlink | busybox --help, apk --version | Layer 1 |
| 4. APK Database | apk info, package count | Layer 3 |
| 5. DNS | UDP to 10.0.2.3:53, parse A record | — |
| 6. TCP HTTP | connect + GET APKINDEX.tar.gz | Layer 5 |
| 7. apk update | full package index download | Layers 3+6 |

If a layer fails, downstream layers are skipped with clear reporting.
The mount key fix unblocked layers 1-2. Layers 3-7 exercise chroot,
dynamic linking, DNS, TCP, and the full Alpine package manager.

## Debug cleanup

The networking investigation from prior sessions left scattered debug
logging across the kernel:
- `POP_COUNT` + warn in virtio-net IRQ handler
- `RX_COUNT` + packet parser in smoltcp receive path
- Interface IP dump in UDP sendto

All removed. The permanent improvements (rx_virtq notify fix, UDP
connect, process_packets calls, deferred job timer integration) stay.
