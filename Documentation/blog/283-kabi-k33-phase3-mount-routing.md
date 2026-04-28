# 283 — kABI K33 Phase 3: mount(2) routing + struct introspection

Phase 2c ended with erofs's `init_module` returning 0 and our
kABI fs registry holding one entry.  But "registered" is half
the journey.  The other half is the inverse: when userspace
runs `mount -t erofs ...`, the kernel's `mount(2)` syscall has
to dispatch through that registry into the .ko module's mount
operation, walk the returned super_block, and adapt it to
Kevlar's existing `kevlar_vfs::FileSystem` trait.

Phase 3 lands the routing layer.  Phase 3b reads the registered
`file_system_type` struct to confirm field offsets work.
Phase 3c (next) will actually call the function pointer.  This
is the post about Phase 3 + 3b — the wire-up before the
dispatch.

## The shape of mount(2) inside Kevlar

Kevlar's `kernel/syscalls/mount.rs` is a switch on `fstype`:

```rust
let fs: Arc<dyn FileSystem> = match fstype {
    "proc" => PROC_FS.clone(),
    "sysfs" => SYS_FS.clone(),
    "tmpfs" => Arc::new(TmpFs::new()),
    "ext2" | "ext3" | "ext4" => kevlar_ext2::mount_ext2()?,
    "devtmpfs" => DEV_FS.clone(),
    "cgroup2" | "cgroup" => CgroupFs::new_or_get(),
    _ => return Err(Errno::ENODEV.into()),
};
```

Adding kABI-routed filesystems means inserting a new arm that
talks to `kabi::fs_register::lookup_fstype(name)`.  v1:

```rust
"erofs" => {
    match crate::kabi::fs_adapter::kabi_mount_filesystem(
        fstype, None, flags as u32, core::ptr::null(),
    ) {
        Ok(fs) => fs,
        Err(e) => {
            debug_warn!("mount: kABI route for {} failed: {:?}",
                        fstype, e);
            return Err(e);
        }
    }
}
```

Note the deliberate boundary: `ext2`/`ext3`/`ext4` stay on the
homegrown `kevlar_ext2` for now, with a TODO comment for when
ext4.ko is built.  The kABI path is opt-in per-fstype, not a
wholesale switch — gives a clean rollback story if a future
.ko's mount path turns out to deadlock or panic.

## The KabiFileSystem adapter

A new file, `kernel/kabi/fs_adapter.rs`:

```rust
pub struct KabiFileSystem {
    super_block: *mut c_void,
    name: alloc::string::String,
}

unsafe impl Send for KabiFileSystem {}
unsafe impl Sync for KabiFileSystem {}

impl FileSystem for KabiFileSystem {
    fn root_dir(&self) -> VfsResult<Arc<dyn Directory>> {
        log::warn!(
            "kabi: KabiFileSystem({}).root_dir() — not yet implemented \
             (super_block={:p}); Phase 3c lands the dentry walk",
            self.name, self.super_block,
        );
        Err(Error::new(Errno::ENOSYS))
    }
}
```

The struct stores a Linux `super_block *` opaque to us.  The
trait impl returns ENOSYS — the dentry-walk that maps Linux's
super_block-→s_root-→d_inode chain to a `kevlar_vfs::Directory`
is Phase 3c work.

The `Send + Sync` impls are unsafe because the super_block is
a raw pointer to module memory.  v1 sidesteps the question:
we never actually access it.

## kabi_mount_filesystem and registry-side struct introspection

The mount entry point looks up the registered filesystem and
reads its `file_system_type` fields:

```rust
const FST_NAME_OFF: usize = 0;
const FST_FS_FLAGS_OFF: usize = 8;
const FST_INIT_FS_CONTEXT_OFF: usize = 16;
const FST_MOUNT_OFF: usize = 32;
const FST_KILL_SB_OFF: usize = 40;

pub fn kabi_mount_filesystem(
    name: &str, _source: Option<&str>,
    _flags: u32, _data: *const u8,
) -> Result<Arc<dyn FileSystem>> {
    let fs_type = match super::fs_register::lookup_fstype(name.as_bytes()) {
        Some(p) => p,
        None => return Err(Errno::ENODEV.into()),
    };
    let fs_type_u8 = fs_type as *const u8;
    let stored_name_ptr =
        unsafe { *(fs_type_u8.add(FST_NAME_OFF) as *const *const u8) };
    let fs_flags = unsafe { *(fs_type_u8.add(FST_FS_FLAGS_OFF) as *const i32) };
    let init_fs_context_ptr =
        unsafe { *(fs_type_u8.add(FST_INIT_FS_CONTEXT_OFF) as *const usize) };
    let mount_op_ptr =
        unsafe { *(fs_type_u8.add(FST_MOUNT_OFF) as *const usize) };
    ...
}
```

This is the moment of truth: do our offset constants match
Linux 7.0.0-14's actual struct layout?  Run with
`kabi-load-erofs=1`:

```
kabi: file_system_type(erofs):
      name="erofs"
      fs_flags=0x21
      init_fs_context=0xffff00007cdc2928
      mount=0xffff00007cdc2bf8
      kill_sb=0xffff00007cdddb00
```

Yes:

  * `name` reads back as "erofs" — the registered string.
  * `fs_flags=0x21` = `FS_REQUIRES_DEV (0x1)` |
    `FS_ALLOW_IDMAPPED (0x20)`, exactly what
    `fs/erofs/super.c` declares for `erofs_fs_type`.
  * Both `init_fs_context` and `->mount` are non-null —
    pointers into the module's loaded `.text` section
    (`0xffff00007cdc...` is in the kABI-loader's allocated
    region for erofs).

Notably, both function pointers are populated.  Modern Linux
fs's are supposed to use only `init_fs_context`, but erofs
still ships a legacy `->mount` thunk.  Phase 3c can pick
either to dispatch — direct `->mount` is simpler (one
function call, no fs_context state machine), so that's
the v1 plan.

## A boot-time probe

Validating Phase 3 wire-up doesn't need a userspace `mount`
call.  Just probe from main.rs after the .ko is loaded:

```rust
match kabi::fs_adapter::kabi_mount_filesystem(
    "erofs", None, 0, core::ptr::null(),
) {
    Ok(_) => info!("kabi: erofs mount route Ok (unexpected at v1)"),
    Err(e) => info!(
        "kabi: erofs mount route returned {:?} (Phase 3 v1 expected)",
        e,
    ),
}
```

Output:

```
kabi: kabi_mount_filesystem(erofs): registry hit, dispatch to
      module ->mount op not yet implemented
kabi: erofs mount route returned ENOSYS (Phase 3 v1 expected)
```

5/5 LXDE 8/8 with the probe + erofs load enabled.  Default
boot still 8/8.  No regressions.

## Why this matters even though it returns ENOSYS

Phase 3 doesn't *do* anything observable from userspace
yet — the mount route hits ENOSYS, no actual mount happens.
But it's the seam between the kABI loader (which proved we
can load Linux fs binaries) and the Kevlar VFS (which
proved we can mount filesystems).

Three durable wins land:

  1.  **The struct layout is verified.**  Reading offset 0
      gives the right name string.  Reading offset 8 gives
      the right `fs_flags` for erofs.  This isn't trivial —
      Linux's structs evolve; a layout mismatch would
      manifest as null-deref on the first field access.
      Pinning to Linux 7.0.0-14 means we own the offset
      constants for that exact .ko version, and we can add
      version detection later if we want to bridge to
      multiple Linux versions.

  2.  **The dispatch boundary is clean.**  mount.rs has one
      arm per kABI-routed fstype.  Adding `xfs`, `btrfs`,
      `9p` later is a four-line diff each.  ext4 is gated
      behind task #99 (the .ko build) but the routing is
      ready.

  3.  **The adapter trait shape is locked.**  Whatever
      Phase 3c does with the `super_block *` produces an
      `Arc<dyn FileSystem>`.  That's what mount.rs expects,
      what `kevlar_vfs` consumes, what every existing
      Kevlar fs (proc/sys/tmpfs/cgroup/devfs) already
      implements.  No VFS-level redesign needed.

## What Phase 3c needs

The TODO list to make a real mount happen:

  * Call the function pointer at `mount_op_ptr` with
    `(fs_type, MS_RDONLY, "/dev/blkdev", null)`.

  * Implement `bdev_file_open_by_path("/dev/blkdev", ...)`
    to return a synthetic `block_device *` wrapping
    `exts/virtio_blk` — this is the kABI block surface
    that's currently a null-returning stub.

  * The function pointer returns a `struct dentry *` or an
    error pointer.  Linux uses high-bit-encoded errors
    (`ERR_PTR(-12)` = `0xfff...ff4`).  Decode and convert
    to `Result`.

  * From the returned dentry: `dentry->d_sb` is the
    super_block, `dentry->d_inode` is the root inode.
    Wrap that in `KabiFileSystem` and `root_dir()` returns
    something useful.

  * For testing: a tiny erofs disk image
    (`mkfs.erofs -d 0 small/ test.erofs`) dropped in the
    initramfs at `/lib/test-erofs.img`, then a kernel-side
    `sys_mount("/lib/test-erofs.img", "/mnt", "erofs", ...)`
    after the .ko load.

Each of those is a separate commit's worth of work.  Phase 3c
will land them one at a time so there's a working bisect
target if any of them breaks.

## Status

| Milestone | Status |
|---|---|
| K1-K29: kABI arc | ✅ |
| K30: graphical Alpine LXDE | ✅ |
| K31-K32 | ✅ |
| K33 Phase 1: gvfs fix; mm bug instrumented; fs scaffolding | ✅ |
| K33 Phase 2: erofs.ko 204 → 0 unresolved | ✅ |
| K33 Phase 2b/2c: erofs init = 0; fs registered = 1 | ✅ |
| K33 Phase 3 v1: mount.rs routes erofs through kabi_mount_filesystem | ✅ |
| K33 Phase 3b: file_system_type struct fields read correctly | ✅ |
| K33 Phase 3c: ->mount dispatch + super_block walk | ⏳ |
| K34+: ext4.ko build | ⏳ |

The interesting moment of K33 is now narrowed to one
function-pointer call away.
