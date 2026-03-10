# Phase 7: Integration Testing

**Goal:** Validate all M5 subsystems working together: boot the kernel, mount
an ext2 disk image via VirtIO, load and execute a program from disk, serve
files over the network.

## Test Strategy

### Test 1: Disk Discovery and Mount

```c
// Detect VirtIO block device, mount ext2
int r = mount("/dev/vda", "/mnt", "ext2", MS_RDONLY, NULL);
assert(r == 0);

// Verify filesystem contents
int fd = open("/mnt/greeting.txt", O_RDONLY);
char buf[64];
int n = read(fd, buf, sizeof(buf));
close(fd);
assert(strncmp(buf, "hello from ext2", 15) == 0);
```

### Test 2: Execute from Disk

```c
// Run a static binary from the ext2 filesystem
pid_t child = fork();
if (child == 0) {
    execve("/mnt/test-binary", argv, envp);
    _exit(127);
}
int status;
waitpid(child, &status, 0);
assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
```

### Test 3: File Metadata

```c
// statfs
struct statfs sfs;
statfs("/mnt", &sfs);
assert(sfs.f_type == 0xEF53);  // EXT2_SUPER_MAGIC
assert(sfs.f_bsize > 0);

// statx
struct statx sx;
statx(AT_FDCWD, "/mnt/greeting.txt", 0, STATX_ALL, &sx);
assert(sx.stx_size == 16);  // "hello from ext2\n"

// utimensat on tmpfs file
struct timespec times[2] = {{.tv_sec = 1000}, {.tv_sec = 2000}};
utimensat(AT_FDCWD, "/tmp/testfile", times, 0);
```

### Test 4: inotify + epoll

```c
int ifd = inotify_init1(IN_CLOEXEC);
int wd = inotify_add_watch(ifd, "/tmp", IN_CREATE | IN_DELETE);

// Add to epoll
int epfd = epoll_create1(EPOLL_CLOEXEC);
struct epoll_event ev = {.events = EPOLLIN, .data.fd = ifd};
epoll_ctl(epfd, EPOLL_CTL_ADD, ifd, &ev);

// Create a file (triggers IN_CREATE)
int fd = open("/tmp/inotify_test", O_CREAT | O_WRONLY, 0644);
close(fd);

// epoll_wait should return the inotify fd as ready
struct epoll_event events[1];
int n = epoll_wait(epfd, events, 1, 100);
assert(n == 1 && events[0].data.fd == ifd);

// Read the inotify event
char buf[256];
n = read(ifd, buf, sizeof(buf));
assert(n > 0);
struct inotify_event *ie = (struct inotify_event *)buf;
assert(ie->mask & IN_CREATE);
```

### Test 5: sendfile

```c
int src = open("/mnt/greeting.txt", O_RDONLY);
int dst = open("/tmp/copy.txt", O_CREAT | O_WRONLY, 0644);
off_t offset = 0;
ssize_t sent = sendfile(dst, src, &offset, 4096);
close(src);
close(dst);
assert(sent == 16);

// Verify copy
int fd = open("/tmp/copy.txt", O_RDONLY);
char buf[64];
int n = read(fd, buf, sizeof(buf));
close(fd);
assert(strncmp(buf, "hello from ext2", 15) == 0);
```

### Test 6: /proc completeness

```c
// /proc/self/maps should list VMAs
int fd = open("/proc/self/maps", O_RDONLY);
char buf[4096];
int n = read(fd, buf, sizeof(buf));
close(fd);
assert(n > 0);
// Should contain "[stack]" somewhere
assert(strstr(buf, "[stack]") != NULL);

// /proc/cpuinfo
fd = open("/proc/cpuinfo", O_RDONLY);
n = read(fd, buf, sizeof(buf) - 1);
close(fd);
buf[n] = '\0';
assert(strstr(buf, "processor") != NULL);
```

## Test Binary: mini_storage

Following the mini_systemd pattern, create `testing/mini_storage.c` that
exercises all M5 features in sequence:

1. File metadata (statfs, utimensat, statx)
2. inotify + epoll
3. sendfile / splice
4. /proc checks (maps, cpuinfo, status, fd/)
5. VirtIO block mount + ext2 read
6. Execute binary from disk
7. Summary: X passed, Y failed

## Build System Changes

### Dockerfile additions

- New build stage for `mini_storage.c`
- New build stage to create ext2 disk image with test files

### Makefile / run-qemu.py additions

- `-drive file=disk.img,format=raw,if=virtio,readonly=on` added to QEMU args
  when a disk image is present
- `make test-storage` target to build and run the full M5 test

## Success Criteria

- All 6 test categories pass
- ext2 files readable and executable from disk
- inotify events delivered via epoll
- /proc/self/maps shows accurate memory layout
- No regressions: mini_systemd and bench still pass
- ARM64: VirtIO-MMIO block device works (same tests)
