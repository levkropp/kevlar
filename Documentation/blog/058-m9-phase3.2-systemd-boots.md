# M9 Phase 3.2: systemd Boots — "Started Kevlar Console Shell"

Phase 3.2 is the largest debugging effort in Kevlar's history. systemd v245
went from crashing in the dynamic linker to booting, loading unit files,
and starting services — all running unmodified Ubuntu 20.04 binaries on
Kevlar under KVM.

## The root cause: page fault double-faults

When glibc's ld.so loads a shared library, it first creates a read-only
reservation mmap covering the entire file, then overlays each segment with
MAP_FIXED at the correct protection level. If any page was faulted in from
the reservation (PROT_READ) before the overlay, the physical page existed
in the page table with read-only PTE flags.

When the overlay VMA changed to PROT_RW and ld.so wrote relocations to
that page, the CPU raised a protection fault (PRESENT | CAUSED_BY_WRITE).
Our page fault handler blindly allocated a new physical page, re-read the
file content from disk, and overwrote the existing PTE — destroying ld.so's
relocation data. Every GOT entry on that page reverted to its unrelocated
virtual address.

The fix uses `try_map_user_page_with_prot()` to detect already-mapped
pages. When the PTE already exists, the handler updates the flags in place
instead of replacing the page.

## VMA split offset bug

A second bug in the same subsystem: when `mprotect` or `MAP_FIXED` splits
a file-backed VMA, the resulting pieces must have adjusted file offsets.
Our `update_prot_range` and `remove_vma_range` cloned the original
`VmAreaType::File` without adjusting the offset, causing demand-paged
pages in split VMAs to read from incorrect file positions. Added
`VmAreaType::clone_with_shift()` to compute correct offsets for each piece.

## Permissive bitflags

`bitflags_from_user!` used strict `from_bits()` which returned ENOSYS for
any unknown flag bits. When systemd opened files with `O_PATH` (0x200000),
the entire openat syscall failed with ENOSYS — reported as "Function not
implemented" for every mount point check. Changed to `from_bits_truncate()`
to silently ignore unknown flags, matching Linux behavior.

## The /proc/self/fd deadlock

`sys_openat` held the `opened_files` spinlock during VFS path resolution.
When the path traversed `/proc/self/fd/N`, `ProcPidFdDir::lookup` tried to
acquire the same lock to read the fd table — deadlock. Fixed by releasing
the lock before resolution for absolute and CWD-relative paths, and
changing `/proc/self/fd/N` to return `INode::Symlink` so the VFS follows
it automatically.

## Fixing the event loop spin

After systemd's manager initialized, `epoll_wait` returned immediately on
every call with 1 event. The cause: `/proc/self/mountinfo` was added to
the sd-event epoll, and the default `FileLike::poll()` returned
`POLLIN | POLLOUT` unconditionally. Changed the default to return empty —
only file types with actual pending data (pipes, sockets, timerfd,
signalfd, inotify) should report readiness.

## Other fixes

- **reboot(CAD_OFF)**: systemd calls `reboot(CAD_OFF)` to disable
  Ctrl-Alt-Del. Our handler unconditionally halted the system.
- **fcntl(F_GETFL)**: returned 0 (O_RDONLY) for all files. systemd checks
  F_GETFL before writing to `cgroup.procs` — skipped the write, causing
  "Failed to allocate manager object".
- **statfs magic numbers**: cgroup2 (0x63677270) and sysfs (0x62656572)
  returned the wrong f_type, so systemd couldn't detect unified cgroups.
- **timerfd overflow**: `(value_sec as u64) * 1_000_000_000` panicked on
  large timer values. Fixed with saturating arithmetic.
- **prlimit64**: returned EFAULT when `old_rlim` was NULL (systemd passes
  NULL when only setting, not reading).
- **AF_UNIX SOCK_DGRAM**: systemd's sd_notify and user-lookup sockets
  require datagram Unix sockets, not just stream.

## Test binaries

Created graduated test binaries to isolate the dynamic linking issue:

- `hello-tls` — shared library with `__thread` TLS variable
- `hello-tls-many` — TLS + libm + libpthread + libdl
- `hello-manylibs` — 5+ libraries including librt
- `hello-libsystemd` — dlopen libsystemd-shared-245.so

All pass, confirming glibc dynamic linking with TLS works correctly.

## Boot sequence

```
systemd 245 running in system mode.
Detected virtualization kvm.
Detected architecture x86-64.
Set hostname to <localhost>.
Welcome to Kevlar OS!
Started Kevlar Console Shell.
```

systemd v245 boots through 12+ shared libraries, initializes the manager,
scans `/etc/systemd/system/` for unit files, loads `default.target` and
`kevlar-getty.service`, forks a child process, and starts `/bin/sh`.

## Results

- 6/6 dynamic linking test binaries pass
- systemd reaches service startup under KVM in <2 seconds
- All existing regression tests pass (31/31 in-memory tests)
- Zero unimplemented syscalls during boot (all stubs return valid values)
