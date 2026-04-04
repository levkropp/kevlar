# Blog 143: First X11 window renders — and the 94-byte buffer overflow that blocked it

**Date:** 2026-04-04
**Milestone:** M11 Alpine Graphical — Phase 6 (X11 Rendering)

## Summary

An xterm window renders on Kevlar's framebuffer. The complete X11
graphics pipeline — kernel framebuffer, PCI BAR mmap, Xorg fbdev
driver, X protocol, client rendering — works end-to-end. Getting
the last mile required fixing a `write_sockaddr` buffer overflow
that corrupted D-Bus's stack, a shebang path resolution bug in
chroots, a missing `iopl` syscall, and a sysfs cross-link that
Xorg needs to map PCI devices to framebuffers.

XFCE session components (xfwm4, panel, session manager) all start
successfully. The desktop currently renders as black because GTK
needs font cache initialization, but the rendering pipeline itself
is proven working.

## The sockaddr overflow

The session D-Bus daemon crashed at a consistent address every run:

```
USER FAULT: GENERAL_PROTECTION_FAULT pid=38 ip=0xa0006d585
```

Disassembling the crash point in musl libc revealed it was
`__stack_chk_fail` — the stack canary check. Something was
overwriting the D-Bus daemon's stack frame.

### Finding the caller

The return address on the stack (`0xa100fc60d`) pointed to
`_dbus_read_socket` in `libdbus-1.so.3`. Disassembling that
function showed:

```asm
; _dbus_read_socket allocates 32 bytes of locals:
sub    $0x20, %rsp
mov    %fs:0x28, %r12        ; load stack canary
mov    %r12, 0x18(%rsp)      ; store at rsp+24
; ...
; buffer for source address at rsp+8 (16 bytes before canary)
; msg_namelen initialized to 16
movl   $0x10, 0x4(%rsp)
lea    0x8(%rsp), %r14       ; addr buffer = rsp+8
```

The function passes a 16-byte stack buffer as the source address
for `recvmsg`. Our kernel's `write_sockaddr` wrote the full
`SockAddrUn` (110 bytes) to this buffer — a **94-byte overflow**
that destroyed the stack canary at `rsp+24`.

### The fix

```rust
// Before: always wrote the full struct regardless of buffer size
dst.write::<SockAddrUn>(sockaddr_un)?;

// After: read caller's max size, truncate to fit
let max_len = socklen.read::<socklen_t>()? as usize;
if max_len >= full_len {
    dst.write::<SockAddrUn>(sockaddr_un)?;
} else if max_len >= 2 {
    // Write family + as much path as fits
    dst.write::<u16>(&family)?;
    dst.add(2).write_bytes(&path[..max_len - 2])?;
}
```

This matches Linux behavior: `recvfrom`/`recvmsg` truncate the
source address to the provided buffer size and write back the full
address length via `msg_namelen`.

## The shebang chroot bug

Shell scripts inside chroots failed with:

```
/bin/sh: can't open '/mnt/usr/bin/startxfce4': No such file or directory
```

The kernel's `execve` handler for `#!` scripts called
`resolve_absolute_path()` to get the script's path for the
interpreter's argv. This returned the host-absolute path
(`/mnt/usr/bin/startxfce4`) instead of the chroot-relative path
(`/usr/bin/startxfce4`).

**Fix:** Use the original `argv[0]` (already chroot-relative)
instead of resolving through the VFS.

## The iopl syscall

Xorg calls `iopl(3)` at startup to access VGA I/O ports via `in`
and `out` instructions. Without kernel support, Xorg hung silently
during initialization — no log file, no error output, just a
blocked process that eventually got killed.

**Diagnosis:** Adding per-syscall logging for Xorg's PID showed
it making syscalls normally until a `hlt` instruction in the
dynamic linker (from `__stack_chk_fail` — a different instance
of the sockaddr bug). But even after fixing that, Xorg hung.
The hang was because `iopl` returned ENOSYS, and Xorg's error
path tried to write to a pipe that was already closed (SIGPIPE).

**Fix:** Implement `iopl(2)` by setting the IOPL bits (12-13) in
the saved RFLAGS on the syscall stack:

```rust
172 /* iopl */ => {
    let level = a1 & 3;
    let old_flags = self.frame.rflags;
    self.frame.rflags = (old_flags & !(3u64 << 12)) | ((level as u64) << 12);
    Ok(0)
}
```

## The PCI-framebuffer sysfs link

Even with `/dev/fb0` working perfectly (ioctls, mmap, read/write
all verified), Xorg's fbdev driver couldn't find it. Adding
`open()` syscall logging revealed the probe path:

```
open: pid=16 path="/sys/bus/pci/devices/0000:00:02.0/graphics/fb0"    ENOENT
open: pid=16 path="/sys/bus/pci/devices/0000:00:02.0/graphics:fb0"    ENOENT
open: pid=16 path="/sys/bus/pci/devices/0000:00:02.0/graphics/fb1"    ENOENT
...
open: pid=16 path="/sys/bus/pci/devices/0000:00:02.0/graphics:fb7"    ENOENT
```

Xorg's `libpciaccess` uses sysfs cross-links to map PCI devices
to their framebuffer devices. On real Linux, the kernel creates
`/sys/bus/pci/devices/DDDD:BB:SS.F/graphics/fb0` as a symlink.

**Fix:** Add `graphics/fb0` subdirectory with `dev` and `uevent`
files inside the PCI device's sysfs entry.

## Visual proof

QEMU's QMP `screendump` command captures the framebuffer state:

| Screenshot | What it shows |
|-----------|---------------|
| Kernel boot | Dark blue test pattern (0xFF1A1A2E) |
| fb0_probe | Red pixel at (0,0) via mmap |
| Xorg running | Black screen + white mouse cursor |
| xterm window | **White terminal window with text cursor** |

The xterm screenshot proves the full pipeline:
1. Kernel paints test pattern to VGA BAR via direct map
2. Xorg clears framebuffer to black via mmap'd `/dev/fb0`
3. Xorg draws software cursor
4. xterm creates window, Xorg composites it to framebuffer
5. QEMU reads VGA BAR and displays in its window

## Investigation tools used

- **Per-PID open() logging:** Added `info!()` in the `SYS_OPEN`
  handler for PIDs > 10, showing the exact file path Xorg tried
  to access. This revealed both the sysfs cross-link issue and
  the shebang chroot path bug.

- **VMA dump on crash:** Added `dump_vma_map()` to the user fault
  handler, printing all VMAs with addresses and permissions. This
  let us map the crash IP to the correct shared library.

- **Library disassembly:** Extracted `libdbus-1.so.3` and
  `libc.musl-x86_64.so.1` from the Alpine disk image via
  `debugfs`, then used `objdump` to decode the crashing
  instruction and its calling function.

- **QEMU QMP screenshots:** Built a Python script that connects
  to QEMU's QMP socket and issues `screendump` commands at
  timed intervals during automated tests.

## Results

```
TEST_PASS mount_rootfs
TEST_PASS dev_fb0_exists
TEST_PASS fb0_ioctl         (1024x768 32bpp)
TEST_PASS dbus_start
TEST_PASS xorg_running
TEST_PASS xdpyinfo
TEST_PASS xterm_running     (visible window on framebuffer!)
TEST_PASS xfwm4_running
TEST_PASS xfce4_panel_running
TEST_PASS xfce4_session_running
```

## Next steps

The XFCE desktop processes run but render as black. The xterm
window renders correctly, proving the graphics stack works. The
remaining issue is GTK theme/font initialization:

1. **Font cache:** `fc-cache -f` needs to run before GTK apps.
   Currently runs in the test but may need optimization.
2. **GTK theme:** Adwaita theme CSS needs to load from
   `XDG_DATA_DIRS`. Setting `GTK_THEME=Adwaita` helps but
   the theme engine itself may need additional setup.
3. **Icon theme:** Panel and desktop icons need the hicolor/Adwaita
   icon theme index files generated by `gtk-update-icon-cache`.
