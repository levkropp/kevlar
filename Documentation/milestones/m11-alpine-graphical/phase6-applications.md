# M11 Phase 6: Applications

**Goal:** Usable applications — terminal, web browser, text editor,
file manager.

## Applications

### Terminal emulator
- **xfce4-terminal** or **foot** (Wayland) — needs PTY (done),
  font rendering (userspace), X11/Wayland client

### Web browser
- **NetSurf** (~5MB) — minimal, no heavy deps. Good first target.
- **Firefox** (~200MB) — real browser, needs many syscalls.
  JIT compilation needs `mmap(PROT_WRITE | PROT_EXEC)` or
  W^X via `mprotect()` (done).

### File manager
- **Thunar** (XFCE) or **PCManFM** — needs inotify (done), stat (done)

### Text editor
- **Mousepad** (XFCE) or **xed** — simple GUI editor
- **nano** / **vim** — terminal editors (should already work)

## Additional Kernel Support

### mmap MAP_SHARED (if not done in Phase 5)

Browser and many apps use shared memory for IPC with the compositor.
MAP_SHARED is the most critical missing memory management feature.

### pipe2(O_DIRECT)

Some applications use O_DIRECT pipes for packet-mode IPC.
Stub returning EINVAL is acceptable.

### memfd_create

Already implemented (M9 Phase 1). Used by Wayland for buffer sharing.

### F_SEAL* (fcntl)

memfd sealing prevents modification after sharing. Used by Wayland.
- `F_ADD_SEALS` / `F_GET_SEALS` — seal write/shrink/grow/future-write
- Can stub (return success without enforcing)

## Verification

```
# Open terminal, run commands
xfce4-terminal -e "uname -a"  # shows Kevlar
# Open file manager
thunar /  # browse root filesystem
# Open web browser
netsurf http://example.com  # renders HTML page
```
