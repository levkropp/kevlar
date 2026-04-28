# 275 — kABI K28: real bytes

K28 lands.  Userspace allocated a 3 MB framebuffer through
Kevlar's DRM ioctl path, mmap'd it, wrote `0xCAFEF00D` and
`0xDEADBEEF` into the first two pixels, read them back through
the same mapping, and bound a real GEM handle to an `fb_id`.

```
USERSPACE-DRM: dumb handle=1 pitch=4096 size=3145728
USERSPACE-DRM: mapdumb offset=0x0
USERSPACE-DRM: drew pattern[0]=0xcafef00d [1]=0xdeadbeef
USERSPACE-DRM: addfb2(handle) fb_id=2
```

`make ARCH=arm64 test-userspace-drm` is the new combined
regression target.  **29 kABI tests** pass.  K27's ghost
modesetting is now ghost-modesetting-with-real-bytes-behind-it:
the framebuffer exists, the pixels are computable, the
userspace tool can draw.  The only thing missing is sending
those bytes to a screen.

## What changed under the hood

Up to K27, every `fb_id` was synthetic.  ADDFB2 accepted any
GEM handle (including 0); userspace had no way to mmap a
buffer; the modesetting pipe was a series of state-recording
ioctls with nothing behind them.

K28 wires three pieces:

**1. The pool.**  At boot, after kABI module loads but before
any DRM probe fires, we eagerly allocate a 4 MB contiguous
physical region:

```rust
const DUMB_POOL_SIZE_BYTES: usize = 4 * 1024 * 1024;
const DUMB_POOL_PAGES: usize = DUMB_POOL_SIZE_BYTES / 4096;  // 1024

let pa = alloc_pages(DUMB_POOL_PAGES, AllocPageFlags::KERNEL)?;
let va = pa.as_vaddr().value();
```

1024 pages, contiguous.  Just under the buddy allocator's
max-order limit (8 MB / 2048 pages).  Big enough for one
1024×768 XRGB8888 frame (3 MB).  Eager init means the pool's
phys-base is known before `drm_dev_register` installs
`/dev/dri/card0`.

**2. The handle table.**  `MODE_CREATE_DUMB` bump-allocates
from the pool, registers a `(handle, offset, size)` tuple:

```rust
fn drm_ioctl_mode_create_dumb(arg: usize) -> isize {
    let mut cmd = read::<DrmModeCreateDumb>(arg);
    let pitch = align_up(cmd.width * cmd.bpp/8, 64);
    let size = align_up(pitch * cmd.height, 4096);

    let mut pool = DUMB_POOL.lock();
    let pool = pool.as_mut().ok_or(-ENOMEM)?;
    if pool.next_offset + size > pool.pool_size { return -ENOMEM; }

    let handle = pool.next_handle;
    pool.next_handle += 1;
    let offset = pool.next_offset;
    pool.next_offset += size;
    pool.handles.push(DumbHandle { handle, offset, size });

    cmd.handle = handle;
    cmd.pitch = pitch as u32;
    cmd.size = size as u64;
    write(arg, cmd);
    0
}
```

Bump allocator.  No reuse, no free.  When DESTROY_DUMB lands
in K30+, we'll add a real allocator.  For K28's "make this
work end-to-end" goal, bump is enough.

**3. The mmap path.**  The trickiest piece.  Kevlar's existing
`sys_mmap` already supports device-memory mmap via a
`mmap_phys_base()` method on `FileLike`.  `/dev/fb0` uses it.
We needed `/dev/dri/cardN` to use it too.

The cleanest fit was extending `KabiCharDevFile`:

```rust
struct KabiCharDevFile {
    fops: *const FileOperationsShim,
    name: String,
    rdev: u32,
    mmap_phys_base: Option<usize>,  // ← K28 addition
}

impl FileLike for KabiCharDevFile {
    fn mmap_phys_base(&self) -> Option<usize> {
        self.mmap_phys_base
    }
}
```

When `drm_dev_register` calls `install_chrdev_in_subdir`, it
passes `Some(DUMB_POOL.base_pa)`.  When userspace calls
`mmap(fd, size, ..., offset)` with our DRM card fd, Kevlar's
`sys_mmap` picks the `VmAreaType::DeviceMemory` branch and
maps `phys_base + offset` into the user's address space.  The
offset is whatever `MAP_DUMB` told the user.  The bytes line
up.

## The round trip

The userspace test does this in 25 lines of C:

```c
struct drm_mode_create_dumb cdumb = {
    .width = 1024, .height = 768, .bpp = 32,
};
ioctl(fd, DRM_IOCTL_MODE_CREATE_DUMB, &cdumb);
// → handle=1, pitch=4096, size=3145728

struct drm_mode_map_dumb mdumb = { .handle = cdumb.handle };
ioctl(fd, DRM_IOCTL_MODE_MAP_DUMB, &mdumb);
// → offset=0

void *ptr = mmap(NULL, cdumb.size, PROT_READ | PROT_WRITE,
                 MAP_SHARED, fd, mdumb.offset);
// → returns a mapped CPU pointer.  Kevlar set up page table
//   entries for 768 pages of pool memory.

volatile uint32_t *p = ptr;
p[0] = 0xCAFEF00D;
p[1] = 0xDEADBEEF;

uint32_t v0 = p[0];
uint32_t v1 = p[1];
// → reads back what was written.  The bytes traveled through
//   userspace MMU → physical pool memory → kernel-resident
//   bytes → userspace MMU → user buffer.
```

Both reads produce the values written.  Through every layer
this entails:

- libc's `ioctl(2)` syscall trap.
- Kevlar's `sys_ioctl` → VFS dispatch → KabiCharDevFile →
  DRM_FOPS_ADAPTER → drm_ioctl → `drm_ioctl_mode_create_dumb`.
- libc's `mmap(2)` syscall trap.
- Kevlar's `sys_mmap` → file-backed VmAreaType branch →
  `KabiCharDevFile::mmap_phys_base()` → device-memory VM area
  → page-table installation in user's address space.
- Direct user-pointer dereference at userspace; the page-fault
  handler maps the requested page on first touch.
- Same path in reverse for the read.

Six layers each direction.  All of them built incrementally.
None of them broke when bytes started flowing through them.

## What "real" means

The framebuffer is now **really there** in physical memory.
If a DMA engine were configured to read from
`pool.base_pa + 0x0` and scan it onto a CRT, it would see
`0xCAFEF00D 0xDEADBEEF 0x00000000 0x00000000 ...`.  The
pixels are computable.

The only thing missing for visible output is **the DMA part**.
Currently no scanout engine is reading from the pool.
cirrus's BAR0 (the "VRAM" region) is a separate 4 KB zero
buffer set up at K19; nobody copies from the DUMB pool to
BAR0.  K29+ wires that copy (or, more elegantly, makes the
pool's PA be the BAR0 PA so the cirrus driver scans
*directly* from the pool).

But the bytes are real now.  That's the K28 milestone.

## ADDFB2 validates handles

K27 accepted any handle (including 0) and just hand-waved.
K28 looks up `cmd.handles[0]` in the pool's table:

```rust
if cmd.handles[0] != 0 {
    if dumb_handle_lookup(cmd.handles[0]).is_none() {
        return -2;  // ENOENT, like real Linux
    }
}
```

When userspace passes a real handle from CREATE_DUMB, ADDFB2
binds an fb_id to it.  When userspace passes garbage, it gets
ENOENT.  K27's "handle=0 escape hatch" is preserved for the
existing test path (which passed 0 deliberately), but the new
test passes a real handle and we can verify the lookup works.

## Cumulative kABI surface (K1-K28)

~331 exports.  ~50 shim modules.  Six DRM modules loadable.
**Eight DRM ioctls succeed** end-to-end.  /dev/dri/card0 is
fully usable for the allocate-map-draw-modeset sequence (just
not the make-pixels-visible step).

## Status

| Surface | Status |
|---|---|
| K1-K27 | ✅ |
| K28 — CREATE_DUMB + MAP_DUMB + real mmap | ✅ |
| K29+ — visible pixels | ⏳ |

## What K29 looks like

With K28's bytes-in-memory, K29 needs to **make those bytes
appear on screen**.  Three approaches in priority order:

1.  **Repoint cirrus's BAR0 at the pool**.  K19's fake cirrus
    PCI device has BAR0 at PA `0x1_0000_0000` pointing at a
    4 KB zero buffer.  If we point it at our 4 MB pool's PA
    instead, when QEMU's emulated cirrus hardware does its
    DMA from VRAM, it reads our pool.  This makes the pool *be*
    VRAM, and SETCRTC's recorded mode becomes the scanout
    mode.  Probably what makes pixels appear in QEMU's window.
2.  **Write through cirrus's MMIO registers**.  The bochs/
    cirrus emulated hardware also has BAR2 mmio registers
    that, in real hardware, drive scanout.  Issuing the right
    register sequence (mode select, vertical/horizontal
    timing, bytes-per-pixel) might be required even with the
    BAR0 redirect.  Look at what real cirrus's
    `cirrus_pipe_enable_vblank` does and replicate the
    `cirrus_dispi_*` register writes.
3.  **fbcon binding**.  Linux's fbcon hooks into a DRM
    device's modeset and writes the kernel console to the
    framebuffer.  Independent of the DRM userspace path.
    Smaller payoff than full Xorg.

(1) is the most direct path to "bytes from the pool become
visible pixels."  (2) may be required to actually start
scanout.  (3) gets us kernel-printk'on-the-screen as a
secondary path.

The "graphical ASAP" arc is now **~2-3 milestones away** from
visible pixels.
