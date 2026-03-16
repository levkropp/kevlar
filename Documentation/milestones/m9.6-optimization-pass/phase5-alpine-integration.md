# M9.6 Phase 5: Alpine Integration (Layers 3-7)

**Blocker:** Layer 3 hangs (chroot + exec dynamically-linked binary from ext2)
**Target:** All 7 layers passing

## Current state

| Layer | Tests | Status |
|-------|-------|--------|
| 1. Foundation | ext2 mount, file existence, stat | PASS (7/7) |
| 2. ext2 Write | create, mkdir, symlink, rename, large file | PASS (5/5) |
| 3. chroot + Dynlink | busybox --help, apk --version | **HANG** |
| 4. APK Database | apk info, package count | blocked by L3 |
| 5. DNS | UDP to 10.0.2.3:53, parse A record | untested |
| 6. TCP HTTP | connect + GET APKINDEX.tar.gz | blocked by L5 |
| 7. apk update | full package index download | blocked by L3+L6 |

## Layer 3 hang analysis

The test does:
```c
chroot("/mnt");    // Alpine ext2 rootfs
chdir("/");
execve("/bin/busybox", {"busybox", "--help"}, NULL);
```

The hang is in `chroot_exec_capture()` which forks a child, has it
chroot+exec, and reads output from a pipe with a 10-second timeout.
The child never produces output and never exits.

### Possible causes

1. **ext2 read path for ELF loading** — execve reads the ELF header
   and program headers from the ext2 filesystem.  If ext2's `read()`
   blocks or returns wrong data, the ELF loader fails silently.

2. **Dynamic linker resolution in chroot** — Alpine's busybox is
   dynamically linked against musl.  The dynamic linker
   `/lib/ld-musl-x86_64.so.1` must be found inside the chroot.
   If the chroot path resolution doesn't correctly translate paths,
   the linker isn't found and execve fails with ENOENT — but the
   child writes to the pipe before exec, so we'd see "C:exec_fail=2".

3. **ext2 demand paging** — the ELF text/data segments are mmap'd
   from the ext2 file.  Page faults during execution read from ext2.
   If ext2's block read is broken for demand-paged regions, the
   process hangs on a page fault.

4. **Pipe read/write after chroot** — the child dup2's the pipe fd
   for stdout/stderr before chroot.  After chroot, the pipe should
   still work (it's an fd, not a path).  But verify the pipe
   implementation handles this correctly.

### Investigation plan

1. Add debug tracing to execve to log each step (ELF parse, mmap,
   entry point) when running inside a chroot
2. Check if the child process reaches the execve or hangs in chroot/chdir
3. If execve succeeds, check if the dynamic linker loads correctly
4. If the linker loads, check if busybox's `--help` handler runs

## Layers 5-6: networking

Layer 5 (DNS) and Layer 6 (TCP HTTP) test the network stack.  These
are independent of Layer 3 and can be tested separately.  The network
stack was partially debugged in a prior session (virtio-net rx_virtq
notify fix, UDP connect, smoltcp integration).

Key questions:
- Does DNS resolution work (UDP to QEMU's built-in DNS at 10.0.2.3)?
- Does TCP connect + HTTP GET work to download APKINDEX.tar.gz?
- Are there smoltcp configuration issues with the QEMU network?

## Success criteria

- All 7 Alpine test layers passing
- `test-alpine` completes within the 180-second timeout
