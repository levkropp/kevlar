# Blog 120: Mount namespace sharing, msync, waitpid fix, and cgroups investigation

**Date:** 2026-03-25
**Milestone:** M10 Alpine Linux

## Summary

Four fixes and one investigation that advance Alpine Linux compatibility:

1. **Mount namespace sharing across fork** — mounts done by child processes are
   now visible to the parent (POSIX semantics), fixing the "Read-only file
   system" failure in APK package installation.
2. **msync(2) implementation** — synchronize file-backed shared mappings back
   to the underlying file.
3. **waitpid/wait4 hang fix** — `JOIN_WAIT_QUEUE.wake_all()` now fires
   unconditionally in `Process::exit()`, even when SIGCHLD disposition is
   Ignore.
4. **OpenRC service enablement** — enabled devfs, sysfs, hostname, bootmisc,
   sysctl, seedrng and other services in the Alpine boot image.
5. **Cgroups v2 investigation** — identified a hang when dynamically-linked
   binaries run from non-root cgroups; deferred until the root cause is fixed.

## Mount namespace sharing

### The bug

When a process calls `fork()`, the child should share the parent's mount
table. If the child runs `mount /dev/sda1 /mnt`, the parent should see `/mnt`
populated. This is standard POSIX behavior — mount namespaces are only
separated by `unshare(CLONE_NEWNS)`.

Kevlar's `RootFs` struct stored mount points as a plain `Vec`:

```rust
pub struct RootFs {
    root_path: Arc<PathComponent>,
    cwd_path: Arc<PathComponent>,
    mount_points: Vec<(MountKey, MountPoint)>,  // deep-cloned on fork!
}
```

Since `RootFs` derives `Clone`, fork created a completely independent copy of
the mount table. Any mounts performed by child processes (like `busybox mount`
called from an init script) were invisible to the parent — breaking Alpine's
boot sequence where OpenRC forks helpers that mount filesystems.

The symptom was APK failing with "Read-only file system" because the ext4
mount done by a child process never appeared in the parent's mount table.

### The fix

Change `mount_points` to `Arc<SpinLock<Vec<(MountKey, MountPoint)>>>`:

```rust
pub struct RootFs {
    root_path: Arc<PathComponent>,     // per-process (chdir is independent)
    cwd_path: Arc<PathComponent>,      // per-process
    mount_points: Arc<SpinLock<Vec<(MountKey, MountPoint)>>>,  // shared via Arc
}
```

When `RootFs` is cloned during fork, `Arc::clone` gives both parent and child
a reference to the **same** mount table. `root_path` and `cwd_path` are still
per-process — `chdir` in the child doesn't affect the parent.

All mount table access methods (`mount()`, `mount_readonly()`,
`get_mount_at_dir()`, `lookup_mount_point()`) now acquire the inner lock via
`lock_no_irq()` to avoid deadlocks with the outer `RootFs` spinlock.

## msync(2)

Implemented the `msync` syscall (number 26 on x86_64, 227 on ARM64) for
synchronizing file-backed shared mappings:

- **MS_SYNC**: Collects dirty pages from MAP_SHARED file-backed VMAs in the
  requested range, then writes them back to the underlying file. Page data is
  read under the VM lock, I/O is performed after releasing it.
- **MS_ASYNC**: Same as MS_SYNC (we don't have a page cache writeback queue).
- **MS_INVALIDATE**: No-op (we don't cache pages independently of the mapping).
- **MAP_PRIVATE**: No-op (writes are private, nothing to sync).

Validation: address must be page-aligned, MS_SYNC and MS_ASYNC are mutually
exclusive, and the range must cover at least one VMA (ENOMEM otherwise).

## waitpid hang fix

### The bug

When a child process exits and SIGCHLD disposition is Ignore (the default
for most processes that don't register a handler), `send_signal(SIGCHLD)`
is a no-op — it skips signals with Ignore disposition. But `wait4`/`waitpid`
still needs to see the child's exit status. The wait queue wake was inside the
`send_signal` success path, so it never fired for Ignore-disposition SIGCHLD.

This caused hangs in Alpine's OpenRC where the init process called `waitpid()`
on children that had already exited but whose exit was never signaled to the
wait queue.

### The fix

Move `JOIN_WAIT_QUEUE.wake_all()` outside the SIGCHLD conditional, so it fires
unconditionally whenever any non-thread process exits:

```rust
if !is_thread {
    if let Some(parent) = current.parent.upgrade() {
        if parent.signals().lock().nocldwait() {
            parent.children().retain(|p| p.pid() != current.pid);
            EXITED_PROCESSES.lock().push(current.clone());
        } else {
            parent.send_signal(SIGCHLD);
        }
    }
    // Always wake waiters — send_signal skips Ignore disposition,
    // but wait4 must still see the child's exit.
    JOIN_WAIT_QUEUE.wake_all();
}
```

## Cgroups v2 investigation

We extended the cgroupfs implementation with `cgroup.events`, `cgroup.kill`,
and `cgroup.freeze` files, and fixed PID 0 handling in `cgroup.procs` writes
(map to current process). This allowed Alpine's OpenRC cgroups service to read
`/proc/self/mountinfo` and detect the cgroup2 filesystem.

However, we discovered a hang when dynamically-linked binaries are executed
from a non-root cgroup. The sequence:

1. OpenRC's cgroups service detects cgroup2 at `/sys/fs/cgroup`
2. It creates a child cgroup and writes the current PID to `cgroup.procs`
3. It then forks and execs Alpine's `/bin/mountinfo` (dynamically linked)
4. The dynamic linker (`ld-musl`) hangs during initialization

Static binaries work fine from any cgroup. The hang appears to be related to
page fault handling or demand paging when the process is in a non-root cgroup.
This needs deeper investigation — we reverted the cgroupfs additions to
maintain a working Alpine boot and will revisit once the root cause is
identified.

## Test results

- **Contract tests:** 159/159 PASS
- **Alpine APK tests:** 29/29 PASS (mount sharing verified)
- **OpenRC boot:** All three runlevels (sysinit, boot, default) complete

## What's next

1. Fix the dynamic-binary-from-child-cgroup hang
2. Re-enable cgroupfs improvements (cgroup.events, cgroup.kill, cgroup.freeze)
3. Enable the OpenRC cgroups service
4. Blocking TCP connect() timeout (SO_SNDTIMEO)
5. More Alpine package testing (python, nginx, dropbear SSH)
