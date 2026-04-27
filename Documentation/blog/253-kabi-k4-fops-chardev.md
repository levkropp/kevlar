# 253 — kABI K4: file_operations + char-device bridge

K4 lands.  Loaded `.ko` modules can now register a `/dev/foo`
char-device node backed by a Linux-shape `struct
file_operations` table.  Open, read, write, ioctl, release
callbacks dispatch through Kevlar's existing FileLike trait
plumbing.

The proof, end-to-end across all four kABI milestones in one
boot:

```
kabi: loading /lib/modules/k4.ko
kabi: /lib/modules/k4.ko license=Some("GPL") author=Some("Kevlar")
       desc=Some("kABI K4 demo: file_operations + char-device")
kabi: applied 24 relocations (3 trampoline(s))
[mod] [k4] init begin
kabi: /dev/k4-demo registered (major=240, minor=0, rdev=0xf000)
[mod] [k4] register_chrdev ok
[mod] [k4] init done
kabi: k4 init_module returned 0
[mod] [k4] open called          ← from kernel-side smoke test
kabi: k4 /dev/k4-demo read 14 bytes: "hello from k4\n"
```

`make ARCH=arm64 test-module-k4` is the regression target.

## Why K4 is the userspace-visible milestone

K1 made modules loadable.  K2 gave them an allocator and a
sleep primitive.  K3 let them register on a bus.  K4 is the
first milestone where the *outside world* sees the loaded
module: a path appears under `/dev/`, opening it dispatches
into the module, reads return bytes the module wrote.

Every Linux char-device driver in the kernel — from `tty` to
`fb` to `random` to `kvm` to `i2c` — uses this same
`register_chrdev` + `struct file_operations` shape.  K4
implements it generically, so any module wanting a
userspace-facing dev node now has a path.

## The shape

The module fills out a static `file_operations` and calls
`register_chrdev`:

```c
static const char K4_MSG[] = "hello from k4\n";

static int k4_open(struct inode *inode, struct file *filp) {
    printk("[k4] open called\n");
    return 0;
}

static ssize_t k4_read(struct file *filp, char *buf,
                       size_t count, loff_t *pos) {
    if (*pos >= 14) return 0;
    size_t n = (count < 14 - *pos) ? count : (14 - *pos);
    if (copy_to_user(buf, K4_MSG + *pos, n) != 0) return -14;
    *pos += n;
    return n;
}

static struct file_operations k4_fops = {
    .open = k4_open,
    .read = k4_read,
};

int init_module(void) {
    register_chrdev(0, "k4-demo", &k4_fops);
    return 0;
}
```

The kernel does the rest:

1. `register_chrdev(0, "k4-demo", &fops)` allocates a
   dynamic major (starting at 240, the Linux experimental
   range).
2. The kABI side allocates a `KabiCharDevFile` adapter
   wrapping the fops pointer + name + rdev.
3. The adapter is `Arc<dyn FileLike>` — Kevlar's existing
   trait for any openable node.
4. We install it at `/dev/k4-demo` via Kevlar's existing
   devfs root (which is just a tmpfs under the hood).

Userspace `open("/dev/k4-demo")` walks the path, finds the
adapter, calls `FileLike::open` — which fires the module's
`fops.open`.  `read(fd)` calls `FileLike::read` — which
fires `fops.read`.  Same for write, ioctl, release.

## The adapter

Most of K4 is one struct + a few hundred lines of trait
methods that translate between FileLike's shape and the C
fops shape.

```rust
struct KabiCharDevFile {
    fops: *const FileOperationsShim,
    name: String,
    rdev: u32,
}

impl FileLike for KabiCharDevFile {
    fn open(&self, _opts: &OpenOptions)
        -> Result<Option<Arc<dyn FileLike>>>
    {
        if let Some(open_fn) = unsafe { (*self.fops).open } {
            let mut inode = InodeShim { /* ... */ };
            let mut filp = FileShim { /* ... */ };
            let rc = open_fn(&mut inode, &mut filp);
            if rc < 0 { return Err(errno_from_neg(rc as isize)); }
        }
        Ok(None)  // continue using `self` for further ops
    }

    fn read(&self, offset: usize, mut buf: UserBufferMut, _: &OpenOptions)
        -> Result<usize>
    {
        let read_fn = match unsafe { (*self.fops).read } {
            Some(f) => f,
            None    => return Err(VfsError::new(VfsErrno::EBADF)),
        };
        let mut tmp: Vec<u8> = vec![0u8; buf.len()];
        let mut filp = FileShim { f_pos: offset as i64, .. };
        let n = read_fn(&mut filp, tmp.as_mut_ptr(), tmp.len(),
                        &raw mut filp.f_pos);
        if n < 0 { return Err(errno_from_neg(n)); }
        let n = n as usize;
        UserBufWriter::from(buf).write_bytes(&tmp[..n])?;
        Ok(n)
    }
    // write, ioctl, stat similar
}
```

The pattern: each FileLike method allocates a transient
`FileShim` + (for open) `InodeShim`, calls the C fop with a
kernel staging buffer, then translates the result back into
the FileLike return type.

## struct file layout

Modules read `filp->private_data` and `filp->f_pos`
extensively (these are the canonical "per-open state" and
"current offset" slots in Linux drivers).  K4's `struct file`
puts them at the same offsets the C header declares:

```c
struct file {
    void           *_kevlar_inner;  // offset  0
    void           *private_data;   // offset  8
    loff_t          f_pos;          // offset 16
    unsigned int    f_flags;        // offset 24
    unsigned int    _pad;
};
```

Same opaque-shim strategy as K2/K3: the kABI internal state
hides behind `_kevlar_inner`, but the fields a module
actually touches live at known offsets.

## The ksym name-collision problem

One unexpected nuisance: my `copy_to_user` shim collided
with an *existing* `copy_to_user` in `kevlar_platform`'s asm
side.

Kevlar's platform code already has `unsafe extern "C" fn
copy_to_user(...)` — declared as an external the asm side
provides.  Adding a new Rust `pub extern "C" fn copy_to_user`
in `kernel/kabi/usercopy.rs` produces two `copy_to_user`
symbols at link time → duplicate symbol error.

The fix needed two changes.  First, the kABI shim got a
non-conflicting Rust name:

```rust
pub extern "C" fn kabi_copy_to_user(...) { /* memcpy */ }
```

Second, a new export macro lets us export under a different
symbol name than the Rust identifier:

```rust
#[macro_export]
macro_rules! ksym_named {
    ($name:literal, $func:ident) => {
        // ...emit a KSym entry with name = $name, addr = $func
    };
}

ksym_named!("copy_to_user", kabi_copy_to_user);
```

Now modules link against `copy_to_user` (the Linux name),
the loader resolves it to `kabi_copy_to_user` (the Rust
implementation), and the platform-side `copy_to_user` (the
real userspace-aware one) is untouched.  This will matter in
K5 when we wire up the *real* user-vaddr-aware copy_to_user
for actual userspace I/O — the kABI shim and the platform
asm coexist behind the same module-visible name.

## copy_to_user is a memcpy in K4

The K4 `copy_to_user` is just `memcpy` — when the K4 fop
runs in our kernel-side smoke test, the "user" pointer it
gets is actually a kernel staging buffer.  No page table
walk, no fault path needed.

Linux's real `copy_to_user` returns "bytes NOT copied" —
non-zero on partial fault — because the user pointer might
be only partially mapped.  K4 always returns 0 (success);
modules that check `if (copy_to_user(...))` see success
every time.

K5 swaps in the real UserVAddr-aware path when the first
userspace-driven read hits a fop.  Until then, the simple
memcpy is sufficient and correct for everything the K4
adapter does.

## Verification

The K4 demo's read flow exercises every layer:

```
kernel/main.rs
    └─ kabi::cdev::read_dev_for_test("k4-demo", &mut buf)
         └─ DEV_FS.root_dir().lookup("k4-demo")
              └─ Arc<KabiCharDevFile>
                   └─ FileLike::open(&opts)
                        └─ fops->open(inode, filp)         ← module's k4_open()
                   └─ FileLike::read(0, buf, &opts)
                        └─ fops->read(filp, tmp, len, pos) ← module's k4_read()
                             └─ copy_to_user(tmp, MSG, n)  ← K4 memcpy shim
```

Five layers of indirection, each translating between
incompatible C and Rust shapes, and the result lands as
`"hello from k4\n"` in our buffer.

## What K4 didn't do

- **Real userspace test.**  No `sys_openat` → `read` → real
  fd plumbing yet.  The K4 verifier opens `/dev/k4-demo`
  from kernel context.  K5 or K6 adds a userspace harness
  binary that gets `open(2)`'d a fd and `read(2)`s through
  the real syscall path.
- **`mmap` fop.**  Drivers that map device memory into
  userspace.  Significant — needs `vm_area_struct` +
  `remap_pfn_range`.  Defers to whichever milestone first
  loads a driver that actually mmap's its device.
- **`poll` fop.**  Modules that need select/poll/epoll
  semantics.  K2's `kabi_wait_event` is the internal
  primitive; K5 wires the FileLike↔fops poll path.
- **`fsync`, `fasync`, `splice_*`, `iterate_*`.**
- **`struct class` + `class_create` + `device_create`.**
  The class-driven /dev/ creation path Linux uses for
  modesetting drivers.  K4 takes the shortcut: install
  directly into devfs root by name.
- **Multi-minor cdevs (count > 1).**  K4 demo is count=1.
- **Real `copy_to_user` against userspace addresses.**

## Cumulative kABI surface (K1 + K2 + K3 + K4)

```
printk
kmalloc kzalloc kcalloc krealloc kfree
vmalloc vzalloc vfree kvmalloc kvzalloc kvfree
init_waitqueue_head destroy_waitqueue_head
wake_up wake_up_all wake_up_interruptible wake_up_interruptible_all
kabi_wait_event
init_completion destroy_completion
complete complete_all wait_for_completion
kabi_init_work schedule_work flush_work cancel_work_sync
kabi_current kabi_current_pid kabi_current_comm
msleep schedule cond_resched schedule_timeout
kref_init kref_get kref_put kref_read
kobject_init kobject_get kobject_put kobject_set_name kobject_add kobject_del
device_initialize device_add device_register device_unregister
get_device put_device dev_set_drvdata dev_get_drvdata
driver_register driver_unregister bus_register bus_unregister
platform_device_register platform_device_unregister
platform_driver_register platform_driver_unregister
__platform_driver_register
platform_set_drvdata platform_get_drvdata
platform_bus_type
alloc_chrdev_region register_chrdev_region unregister_chrdev_region
cdev_init cdev_add cdev_del
register_chrdev unregister_chrdev
copy_to_user copy_from_user clear_user strnlen_user
```

~64 symbols.  Linear scan is still sub-microsecond at this
size; binary search switchover defers to ~200+.

## Status

| Surface | Status |
|---|---|
| K1 — ELF .ko loader | ✅ |
| K2 — kmalloc / wait / work / completion | ✅ |
| K3 — device model + platform bind/probe | ✅ |
| K4 — file_operations + char-device | ✅ |
| K5 — real I/O primitives (vmalloc, dma_alloc, MMIO) | ⏳ next |
| K6-K9 | ⏳ |

## What K5 looks like

K5 brings the *real I/O* primitives — the things a driver
needs once it has to actually talk to hardware.  Three
threads:

- **Real `vmalloc` (non-contiguous physical pages).**  K2's
  vmalloc returns physically contiguous memory because it
  goes through the buddy allocator.  Real drivers (esp.
  network and graphics) need much larger allocations than
  the buddy can give as one chunk; Linux stitches separately-
  allocated pages into a virtual range via `vmap`.  K5 adds
  the real path.
- **`dma_alloc_coherent`.**  Allocate a buffer that's
  reachable by both the CPU and a DMA-capable device,
  page-aligned, with cache coherency guarantees.  Plus
  `dma_map_single` / `dma_unmap_single` for one-shot
  bidirectional buffers.
- **MMIO helpers.**  `ioremap`, `iounmap`, `readb`/`writeb`,
  `readl`/`writel`, `readq`/`writeq` — typed memory-mapped
  I/O accessors that compile to the right load/store with
  the right barriers.

Plus: the real userspace-aware `copy_to_user` path, real
struct-layout faithfulness work begins to land (so prebuilt
Linux modules can start to link), and the K5 demo target —
something like a tiny "fake hardware" platform driver that
allocates DMA-coherent memory, maps an MMIO region (over
QEMU's reserved virtual address space, no real hardware
yet), and performs an end-to-end read/write through the
fake hardware.

K5 is the inflection point where the kABI surface becomes
sufficient to load *real* Linux driver source unchanged.
After K5, K6 is "load Linux's prebuilt `simplefb.ko`" — the
first time a binary `.ko` from the Linux source tree
actually runs in Kevlar.
