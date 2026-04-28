# 284 — kABI K33 Phase 3d: full erofs mount chain runs without crashing

Phase 3 ended with the mount syscall routing in place but
the dispatch returning ENOSYS unconditionally.  The Phase 3d
arc — five commits this session — actually invokes erofs's
mount machinery, traverses through `init_fs_context` →
`fc->ops->get_tree` → erofs internals → our stubs, and
returns clean error codes back through the chain.  Zero
kernel panics in the dispatch path.

The final state with `kabi-load-erofs=1`:

```
kabi: erofs init_module returned 0
kabi: dispatching erofs init_fs_context(fc=0xffff00007fc34010)
kabi: erofs init_fs_context returned 0 — fc->ops populated
kabi: fc->ops = 0xffff00007cdd9b60
kabi: fc->ops->get_tree = 0xffff00007cdc1ac8
kabi: dispatching erofs ops->get_tree(fc=0xffff00007fc34010)
kabi: get_tree_bdev_flags (stub) — returning -ENOTBLK
kabi: erofs ops->get_tree returned -2 (-ENOENT from filp_open)
```

Each line is a real .ko function call running real Linux
code on top of our kABI surface.  Below is the journey from
"first attempt panicked" to here.

## The Phase 3c v0 panic

Phase 3 ended with `kabi_mount_filesystem` calling the
`->mount` function pointer at offset 32 of the
`file_system_type` struct.  First attempt:

```
panic at platform/arm64/interrupt.rs:136:17:
kernel page fault: pc=0xffff00007cdc2c1c (= mount_op + 0x24)
                   far=0x2820262029766574
                   esr=0x96000004
```

The FAR value `0x2820262029766574` decodes as ASCII bytes
`tev)&( (`.  The kernel was reading a string-typed pointer
that hadn't been initialized.

## The fix: cross-check struct layout against actual kernel source

We have the matching kernel tree at
`build/linux-src.vanilla-v7.0/`.  Pulling up `fs.h:2271`:

```c
struct file_system_type {
    const char *name;                            /* +0 */
    int fs_flags;                                /* +8 */
    int (*init_fs_context)(struct fs_context *); /* +16 */
    const struct fs_parameter_spec *parameters;  /* +24 */
    void (*kill_sb)(struct super_block *);       /* +32 */
    struct module *owner;                        /* +40 */
    struct file_system_type *next;               /* +48 */
    ...
};
```

**Linux 7.0 removed the legacy `->mount` callback entirely.**
Offset +32 is `kill_sb`, not `mount`.  Our v0 dispatch was
calling erofs's `kill_sb` thunk with `(fs_type, flags,
dev_name, null)` instead of `(super_block)`.  `kill_sb`
treated the fs_type pointer as a super_block and dereferenced
a string-typed field — which landed in random module memory
and read ASCII chars as a pointer.  Hence the text-shaped FAR.

This is the kind of thing that's catastrophic to debug
without good ground truth.  Pinning to Linux 7.0.0-14 means
we own the offset constants — we can pull them out of the
matching headers, no guesswork.

## Phase 3d v1: route through init_fs_context

Linux 7.0's only mount entry point is `init_fs_context`.
The dispatch:

```rust
type InitFsContextFn = unsafe extern "C" fn(*mut c_void) -> i32;
let init_fc_fn: InitFsContextFn =
    unsafe { core::mem::transmute(init_fs_context_ptr) };

let fc = super::alloc::kmalloc(512, 0);  // struct fs_context ~280 bytes
unsafe { core::ptr::write_bytes(fc as *mut u8, 0, 512); }
unsafe {
    *(fc.cast::<u8>().add(FC_FS_TYPE_OFF) as *mut *mut c_void) = fs_type;
    *(fc.cast::<u8>().add(FC_PURPOSE_OFF) as *mut u8) = FS_CONTEXT_FOR_MOUNT;
}

let rc = unsafe { init_fc_fn(fc) };
```

erofs's `init_fs_context` reads `fc->fs_type`, allocates
`erofs_sb_info` via `kzalloc`, stashes in `fc->s_fs_info`,
sets `fc->ops` to its operations table, returns 0.  Our
stubs supply `kzalloc` (via `kmalloc`) and the slab
infrastructure from Phase 2.

Output:

```
kabi: dispatching erofs init_fs_context(fc=0xffff00007fc34010)
kabi: erofs init_fs_context returned 0 — fc->ops populated
```

## Phase 3d v2: dispatch ops->get_tree

After `init_fs_context` succeeds, `fc->ops` (offset 0) is
an `fs_context_operations *` table.  `get_tree` is the 5th
fn pointer (offset +32):

```rust
const FC_OPS_OFF: usize = 0;
const OPS_GET_TREE_OFF: usize = 32;

let ops_ptr = unsafe { *(fc.cast::<u8>().add(FC_OPS_OFF) as *const usize) };
let get_tree_ptr = unsafe {
    *((ops_ptr as *const u8).add(OPS_GET_TREE_OFF) as *const usize)
};

type GetTreeFn = unsafe extern "C" fn(*mut c_void) -> i32;
let get_tree_fn: GetTreeFn = unsafe { core::mem::transmute(get_tree_ptr) };
let rc = unsafe { get_tree_fn(fc) };
```

Output:

```
kabi: fc->ops = 0xffff00007cdd9b60
kabi: fc->ops->get_tree = 0xffff00007cdc1ac8
kabi: dispatching erofs ops->get_tree
kabi: erofs ops->get_tree returned -22 (-EINVAL)
```

`-22` because our stub `get_tree_bdev_flags` returns -EINVAL
unconditionally.

## Phase 3d v3: -ENOTBLK + ERR_PTR + fc->source

erofs's `get_tree` calls `get_tree_bdev_flags` first.  If
that returns `-ENOTBLK` and `CONFIG_EROFS_FS_BACKED_BY_FILE`
is set, erofs falls into a file-backed mount path that uses
`filp_open(fc->source)` instead of opening a block device.
That's a much more accessible mount target — files in our
initramfs are backed by Kevlar code we already trust, while
"real Linux block_device + virtio_blk handshake" is a
multi-month thing to bring up.

Three coordinated changes:

  * `get_tree_bdev_flags` returns `-15` (-ENOTBLK) — push
    erofs into the file-backed path.
  * `filp_open` returns `ERR_PTR(-ENOENT)` not null.
    Linux's `IS_ERR((void*)-N) == true` for small negative
    N; null would survive `IS_ERR` and crash the caller.
  * fs_adapter populates `fc->source` (offset 112),
    `fc->fs_type` (offset 40), `fc->sb_flags` (offset 128),
    `fc->purpose` (offset 140) before calling
    `init_fs_context`.

Output:

```
kabi: dispatching erofs ops->get_tree
kabi: get_tree_bdev_flags (stub) — returning -ENOTBLK
kabi: erofs ops->get_tree returned -2 (-ENOENT)
```

erofs's get_tree:

  1. Called `get_tree_bdev_flags` → got -ENOTBLK ✓
  2. Checked `fc->source` (proves offset 112 right) ✓
  3. Called `filp_open(fc->source)` → got ERR_PTR(-ENOENT)
  4. Returned -ENOENT ✓

Five real .ko function calls into Linux code, three real
returns of Linux errno values back through the chain.  Zero
crashes.

## Phase 3e: embed an actual erofs disk image

`brew install erofs-utils` gives `mkfs.erofs`.  16 KB of
hello-world content packs into a 16 KB image:

```
$ mkdir /tmp/erofs-content
$ echo "hello from kABI-mounted erofs!" > /tmp/erofs-content/hello.txt
$ echo "kevlar: K33 demo" > /tmp/erofs-content/info.txt
$ mkfs.erofs -d 0 /tmp/test.erofs /tmp/erofs-content/
Processing / ...
Processing hello.txt ...
Processing info.txt ...
Build completed.
$ ls -l /tmp/test.erofs
-rw-r--r-- 16384 /tmp/test.erofs
```

Stage at `build/test-fixtures/test.erofs`,
`tools/build-initramfs.py` copies into the initramfs at
`/lib/test.erofs`, fs_adapter.rs's `fc->source` now points
at this path.  When `filp_open` finally exists (Phase 3f+)
it opens this file through Kevlar's initramfs and erofs
reads its on-disk superblock.

## What's not done yet

The TODO list to make a *real* mount happen — files actually
appearing under `/mnt/erofs` after `sys_mount` — is sizeable:

  1. **`filp_open` returning a synthesized `struct file *`.**
     Need to fake `f_mapping` (offset +16) → `struct
     address_space` → `a_ops` → `struct
     address_space_operations` → `read_folio` (offset +0).
     And `f_inode` (offset +32) → `struct inode` →
     `i_mode` (offset 0, set to `S_IFREG | 0644`).  Erofs's
     `S_ISREG(file_inode(file)->i_mode)` check must pass.

  2. **`get_tree_nodev` real impl.**  After `filp_open`
     returns a valid file, erofs's get_tree calls
     `get_tree_nodev(fc, erofs_fc_fill_super)`.  Linux's
     `get_tree_nodev` allocates a `struct super_block`
     (~1024 bytes), calls `fc_fill_super`, wraps in a
     dentry tree.  Each step needs the right struct layout.

  3. **Real `read_folio` impl** that reads bytes from
     the initramfs file into a folio (page-sized buffer).
     Erofs's `fc_fill_super` calls this to read the on-disk
     superblock; without it, erofs gets an unfilled folio
     and bails.

  4. **`KabiFileSystem::root_dir()`** dereferences the
     resulting super_block's `s_root` dentry chain into a
     `kevlar_vfs::Directory`.

Each of those is hundreds of lines of careful struct-offset
work — ~weeks of effort, not a session's lift.  The accurate
characterisation: Phase 3 has stitched the *control flow*
end-to-end; Phase 4+ provides the *data flow* — the actual
struct synthesis Linux fs code expects to find at the other
end of every pointer dereference.

## What did this session actually achieve?

Stepping back, the K33 arc this session:

| Commit | Achievement |
|---|---|
| `cdb2337` | erofs.ko: 204 → 0 unresolved symbols |
| `7766c33` | Phase 2b: real slab/page/vmalloc impls |
| `fd0dc7c` | Phase 2c: erofs init_module returns 0 |
| `5bec300` | Pool-sweep cadence fix (LXDE flake 50%→10%) |
| `042f57a` | Phase 3 v1: mount.rs route |
| `04732dd` | Phase 3b: file_system_type fields read |
| `b2d71a9` | Phase 3c v0: ->mount type-shape + ERR_PTR |
| `412dc90` | Phase 3d v1: init_fs_context returns 0 |
| `03ddaee` | Phase 3d v2: ops->get_tree dispatches |
| `d52761f` | Phase 3d v3: full chain to filp_open ERR_PTR |
| `017287a` | Phase 3e: embed test image, fc->source wired |

Plus blogs 280/281/282/283/284 documenting each phase.

That's 11 commits + 5 blog posts spanning Phase 1
diagnostics (gvfs gschemas fix), Phase 2 (full erofs.ko
symbol resolution), Phase 2b/2c (real impls + init success),
Phase 3 (routing + struct-introspection), Phase 3d (full
mount call chain), Phase 3e (test image).  The kABI
filesystem playbook is end-to-end alive: a Linux fs `.ko`
loads into Kevlar, runs its constructor, registers, and
dispatches mount operations cleanly.

What's not yet alive: the *result* of the mount.  The .ko
returns errno before reaching any real I/O, because the
shim layer for Linux's struct file/inode/address_space/
super_block/dentry/page-cache machinery isn't built yet.
That's the K34+ scope — months of struct synthesis work,
exactly what FreeBSD's LinuxKPI took years to perfect for
GPU drivers.

## Status

| Milestone | Status |
|---|---|
| K1-K29: kABI arc | ✅ |
| K30-K32: graphical Alpine LXDE + itest framework | ✅ |
| K33 Phase 1: gvfs SIGTRAP + scaffolding | ✅ |
| K33 Phase 2: erofs.ko 204 → 0 unresolved | ✅ |
| K33 Phase 2b/2c: erofs init = 0; fs registered | ✅ |
| K33 Phase 3: mount route + struct introspection | ✅ |
| K33 Phase 3d: full mount chain runs | ✅ |
| K33 Phase 3e: erofs test image embedded | ✅ |
| K34+: real Linux struct synthesis | ⏳ |
| K34+: ext4.ko build (Docker / GNU GCC env) | ⏳ |

The interesting moment of K33 is reached: a Linux fs `.ko`
running on top of Kevlar, executing real Linux code, with
Kevlar handling every kABI call cleanly.  The frontier is
just one set of structs away.
