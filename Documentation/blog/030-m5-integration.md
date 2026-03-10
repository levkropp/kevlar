# M5 Phase 7: Integration Testing — All Systems Go

Milestone 5 is complete. Every subsystem built across Phases 1–6 now works
together in a single integration test: VirtIO block device, ext2 filesystem,
statfs, statx, inotify+epoll, sendfile, exec-from-disk, and /proc. Nine tests,
nine passes.

## What Phase 7 Tests

```
TEST_PASS statfs_ext2      # statfs("/tmp/mnt") returns EXT2_SUPER_MAGIC
TEST_PASS statfs_tmpfs     # statfs("/tmp") returns TMPFS_MAGIC
TEST_PASS statx_size       # statx on ext2 file returns correct stx_size=16
TEST_PASS utimensat_stub   # utimensat returns 0
TEST_PASS inotify_epoll    # IN_CREATE delivered via epoll after open(O_CREAT)
TEST_PASS sendfile_ext2    # sendfile copies ext2 file to tmpfs, content matches
TEST_PASS exec_disk        # fork+execve /tmp/mnt/hello exits 0
TEST_PASS proc_maps        # /proc/self/maps contains [stack]
TEST_PASS proc_cpuinfo     # /proc/cpuinfo contains "processor"
TEST_PASS mini_storage_all # summary: 9 passed, 0 failed
```

Run with:

```sh
make test-storage
```

## The Disk Image Build Pipeline

In Phase 6 the disk image was created manually with `sudo mount`. Phase 7
automates this entirely through Docker.

A new `disk_image` Docker stage uses `mke2fs -d`:

```dockerfile
FROM ubuntu:20.04 AS disk_image
RUN apt-get update && apt-get install -qy e2fsprogs
COPY --from=disk_hello /disk_hello /disk_root/hello
RUN printf 'hello from ext2\n' > /disk_root/greeting.txt && \
    mkdir -p /disk_root/subdir && \
    printf 'nested file\n' > /disk_root/subdir/nested.txt && \
    ln -s greeting.txt /disk_root/link.txt && \
    chmod +x /disk_root/hello && \
    dd if=/dev/zero of=/disk.img bs=1M count=16 2>/dev/null && \
    mke2fs -t ext2 -d /disk_root /disk.img
```

`mke2fs -d <dir>` (e2fsprogs ≥ 1.43) creates a fully-populated ext2 image
from a directory tree — including symlinks, permissions, and binaries. Ubuntu
20.04 ships 1.45.5, so this works out of the box. The Makefile extracts the
image:

```makefile
build/disk.img: testing/Dockerfile testing/disk_hello.c
    docker build --target disk_image -t kevlar-disk-image -f testing/Dockerfile .
    docker create --name kevlar-disk-tmp kevlar-disk-image
    docker cp kevlar-disk-tmp:/disk.img build/disk.img
    docker rm kevlar-disk-tmp
```

The `disk_hello` binary is a 3-line C program that prints "hello from disk!\n"
and exits 0. It exercises the entire path from ext2 block read → ELF loader →
execve → process exit → waitpid status check.

## Bug Found: inotify Not Fired on open(O_CREAT)

The inotify+epoll test immediately revealed a gap: creating a file with
`open(path, O_CREAT | O_WRONLY, ...)` did not deliver an `IN_CREATE` event.

Looking at the code, `mkdir()` and `rename()` both called
`inotify::notify(parent, name, IN_CREATE)` — but `open()` with `O_CREAT` did
not. The fix is one call in `sys_open()`:

```rust
if flags.contains(OpenFlags::O_CREAT) {
    match create_file(path, flags, mode) {
        Ok(_) => {
            // Notify inotify watchers of the new file.
            if let Some((parent, name)) = path.parent_and_basename() {
                inotify::notify(parent.as_str(), name, inotify::IN_CREATE);
            }
        }
        Err(err) if !flags.contains(OpenFlags::O_EXCL)
                 && err.errno() == Errno::EEXIST => {}
        Err(err) => return Err(err),
    }
}
```

With this fix, `open()` and `mkdir()` both deliver `IN_CREATE`. The epoll test
then works correctly: the event is queued before `epoll_wait` is called, so
`epoll_wait` returns immediately.

## statfs Gets filesystem-Aware

Previously `statfs("/tmp/mnt")` returned `TMPFS_MAGIC` (0x01021994) for every
path that wasn't under `/proc`. Phase 7 adds `MountTable::fstype_for_path()`:

```rust
pub fn fstype_for_path(path: &str) -> Option<String> {
    let entries = MOUNT_ENTRIES.lock();
    let mut best_len = 0usize;
    let mut best_fstype: Option<String> = None;
    for entry in entries.iter() {
        let mp = entry.mountpoint.as_str();
        let matches = if mp == "/" {
            true
        } else {
            path.starts_with(mp)
                && (path.len() == mp.len()
                    || path.as_bytes().get(mp.len()) == Some(&b'/'))
        };
        if matches && mp.len() >= best_len {
            best_len = mp.len();
            best_fstype = Some(entry.fstype.clone());
        }
    }
    best_fstype
}
```

The boundary check (`next char == '/'` or exact match) prevents `/tmp/mntfoo`
from matching a mount at `/tmp/mnt`. The longest-prefix match means nested
mounts resolve to their innermost filesystem. `statfs.rs` uses this to return
`EXT2_SUPER_MAGIC` (0xEF53) for paths under any ext2 mount:

```rust
fn for_path(path: &Path) -> StatfsBuf {
    match MountTable::fstype_for_path(path.as_str()).as_deref() {
        Some("proc") | Some("sysfs") => StatfsBuf::procfs(),
        Some("ext2") => StatfsBuf::ext2(),
        _ => StatfsBuf::tmpfs(),
    }
}
```

## exec from Disk

The exec-from-disk test is the culmination of M5:

```c
pid_t child = fork();
if (child == 0) {
    char *argv[] = { "/tmp/mnt/hello", NULL };
    char *envp[] = { NULL };
    execve("/tmp/mnt/hello", argv, envp);
    _exit(127);
}
int status = 0;
waitpid(child, &status, 0);
assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
```

`/tmp/mnt/hello` is a static musl ELF binary stored on the ext2 disk image.
The kernel's execve reads the ELF header from ext2 blocks, maps the PT_LOAD
segments, sets up the stack, and jumps to the entry point. The binary prints
"hello from disk!\n" and returns 0. The parent's `waitpid` confirms it exited
cleanly.

This path touches: VirtIO block I/O → block cache → ext2 block pointer
resolution → VFS FileLike::read → ELF loader → demand-paging → process
execution → wait4 signal delivery. Everything in the chain worked on the
first run.

## M5 Complete

Milestone 5 is done. The storage stack is fully operational:

| Phase | What | Status |
|-------|------|--------|
| 1 | File metadata (stat, statx, statfs, utimensat) | ✓ |
| 2 | inotify (IN_CREATE, IN_DELETE, IN_MOVED) | ✓ |
| 3 | Zero-copy I/O (sendfile, splice, tee) | ✓ |
| 4 | /proc & /sys completeness | ✓ |
| 5 | VirtIO block device driver | ✓ |
| 6 | Read-only ext2 filesystem | ✓ |
| 7 | Integration testing | ✓ |

Next: Milestone 6 — SMP and threading (pthreads, futex, clone, TLS). This is
the last major piece before Wine can run.
