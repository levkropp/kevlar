# 076: Contract Test Expansion — 31 to 86 Tests, 19 Bugs Fixed

## Motivation

Kevlar had 31 contract tests covering ~22% of 118 implemented syscalls. BusyBox
(101 integration tests) provides black-box confidence, but when something breaks
it doesn't pinpoint which syscall has wrong semantics. To establish credible ABI
compatibility evidence before M7 (glibc), we needed much broader contract coverage.

## What we built

55 new standalone C tests across 7 new categories, all auto-discovered by the
existing `compare-contracts.py` infrastructure. No build system changes needed.

| Category | Tests | Syscalls covered |
|---|---|---|
| fd/ | 7 | dup, dup2, dup3, pipe2, fcntl, lseek, readv, writev, sendfile, close_range |
| events/ | 7 | epoll (level + edge), eventfd, timerfd, poll, select, signalfd |
| sockets/ | 7 | socketpair, AF_UNIX stream, getsockopt, shutdown, sendto/recvfrom |
| filesystem/ | 8 | mkdir, rmdir, unlink, rename, symlink, link, getcwd, access, getdents64, statx |
| signals/ + process/ | 7 | execve reset, sigchld+wait, alarm, sigsuspend, setpgid, getuid, prlimit |
| threading/ | 6 | pthread/clone, futex WAIT/WAKE, set_tid_address, robust_list, tgkill, sched_affinity |
| time/ | 7 | clock_gettime (4 clocks), gettimeofday, nanosleep, sysinfo, uname, getrandom |
| vm/ (new) | 6 | munmap partial, mmap file, brk, madvise, MAP_SHARED, mprotect roundtrip |

Every test compiles with `musl-gcc -static -O1`, passes on Linux natively, and
runs on Kevlar via QEMU. The harness compares output line-by-line.

## Bugs found and fixed

The new tests exposed 21 divergences from Linux. We fixed 19:

### FD_CLOEXEC was silently lost on dup3

`dup3(fd, target, O_CLOEXEC)` set the flag on `LocalOpenedFile.close_on_exec`
but `fcntl(F_GETFD)` read from `OpenedFile.options.close_on_exec` — the wrong
copy. The root cause: close-on-exec is a per-fd property (POSIX), but Kevlar
stored it in two places and read the wrong one.

**Fix:** Added `get_cloexec()`/`set_cloexec()` to `OpenedFileTable` that read
the per-fd `LocalOpenedFile.close_on_exec` field directly.

### pipe2 O_NONBLOCK returned EOF instead of EAGAIN

`PipeReader::read()` returned `Ok(0)` (EOF) for nonblock + empty, making
userspace think the writer had closed. POSIX requires `Err(EAGAIN)`.

**Fix:** Split the fast-path check: `closed_by_writer → Ok(0)`, `nonblock →
Err(EAGAIN)`.

### lseek on pipes succeeded silently

Pipes returned `Ok(0)` from lseek instead of `Err(ESPIPE)`. No file type
had a way to declare itself non-seekable.

**Fix:** Added `FileLike::is_seekable()` (default `true`), overridden to
`false` in `PipeReader`/`PipeWriter`/`UnixStream`/`UnixSocket`. `sys_lseek`
checks it before proceeding.

### rename within tmpfs returned EXDEV

The tmpfs `rename()` used `downcast(new_dir)` to get `&Arc<Dir>`, but this hit
the known Arc downcast bug (method resolution picks the blanket `Downcastable`
impl on `Arc<dyn Directory>` itself, not the concrete type inside). Every
same-tmpfs rename failed with EXDEV.

**Fix:** Deref through the Arc before downcasting: `(**new_dir).as_any()
.downcast_ref::<Dir>()`. This dispatches through the vtable to the concrete
type's `Downcastable` impl.

### getdents64 missing "." and ".."

tmpfs `readdir()` only returned real directory entries. POSIX requires
synthetic `.` and `..` entries.

**Fix:** Return `.` at index 0, `..` at index 1, real entries at index-2.

### Hard link didn't update st_nlink

`Dir::link()` inserted the directory entry but never incremented the inode's
link count. `Dir::unlink()` never decremented it.

**Fix:** Added `nlink: AtomicUsize` to tmpfs `File`, increment in `link()`,
decrement in `unlink()`. Uses `(**file_like).as_any().downcast_ref::<File>()`
to work around the Arc downcast bug.

### select() returned before polling fds

`sys_select` with `timeout={0,0}` checked `elapsed >= timeout_ms` (0 >= 0 = true)
before polling any fds, returning 0 immediately. Every zero-timeout select was
a no-op.

**Fix:** Move timeout check after fd polling — always poll once, then check timeout.

### MADV_DONTNEED was a no-op

The madvise stub returned 0 without touching pages. Applications expecting
MADV_DONTNEED to discard anonymous pages (re-zeroed on next access) got stale data.

**Fix:** Walk the page table, unmap each page, free via refcount, flush TLB.

### PipeReader::poll() didn't report EOF

When the write end of a pipe closed, `poll(POLLIN)` returned 0 because it only
checked `buf.is_readable()`. The `closed_by_writer` flag was ignored.

**Fix:** `if inner.buf.is_readable() || inner.closed_by_writer { POLLIN }`.

### CLOCK_REALTIME returned epoch 0

`WALLCLOCK_TICKS` was initialized to 0 at boot and only incremented by timer
IRQs — no real-time reference. `clock_gettime(CLOCK_REALTIME)` always returned
seconds since boot, not since 1970.

**Fix:** Added CMOS RTC reader (`platform/x64/mod.rs::read_rtc_epoch_secs()`)
that reads BCD-encoded date/time from ports 0x70/0x71, converts to Unix epoch,
and stores in `WALLCLOCK_EPOCH_NS` at boot. `read_wall_clock()` adds tick-based
offset to the epoch base.

### SOCK_DGRAM socketpair had wrong SO_TYPE and no message boundaries

`socketpair(AF_UNIX, SOCK_DGRAM, 0)` created SOCK_STREAM sockets internally.
`getsockopt(SO_TYPE)` was hardcoded to return 1 (SOCK_STREAM). DGRAM writes
were concatenated in a continuous ring buffer with no message framing.

**Fix:** Added `sock_type: i32` field to `UnixStream` and `UnixSocket`. The
`socketpair` and `socket` syscalls pass the type through. For DGRAM mode, writes
prepend a 2-byte LE length prefix; reads consume exactly one message per call,
preserving boundaries. `getsockopt(SO_TYPE)` now queries `FileLike::socket_type()`.

### socket() returned ENOSYS for unsupported families

Linux returns EAFNOSUPPORT for unknown address families and EINVAL for bad
socket types within a known family. Kevlar returned ENOSYS for everything,
which would break any code that checks specific errno values.

**Fix:** Match Linux: `EAFNOSUPPORT` for unknown families, `EINVAL` for bad
types within AF_UNIX/AF_INET.

### poll() stripped POLLHUP from revents

`sys_poll` computed `revents = events & status`, which masked out POLLHUP since
userspace only requested POLLIN. Per POSIX, POLLHUP and POLLERR are always
reported regardless of the requested events mask.

**Fix:** `revents = (events & status) | (status & (POLLHUP | POLLERR))`.

### statx mask missing STATX_MNT_ID

Kevlar returned `stx_mask = 0x7ff` (STATX_BASIC_STATS), Linux returns `0x17ff`
(includes STATX_MNT_ID). Any application checking the mask for mount ID support
would see Kevlar as less capable.

**Fix:** Set `stx_mask = STATX_BASIC_STATS | STATX_MNT_ID`.

### uname release version outdated

Kevlar reported kernel release "4.0.0". Updated to "6.19.8" to match the Linux
version we test against. Drivers that version-gate features check this string.

### Other fixes

- **set_robust_list:** Now returns EINVAL for invalid size (was accepting anything)
- **/dev/null poll:** Now reports POLLOUT | POLLIN (was empty PollStatus)
- **alarm remaining:** Fixed integer truncation (`ticks*1M/HZ/1M → (ticks+HZ-1)/HZ`)

## Results

Before:
```
47/86 PASS | 15 XFAIL | 17 DIVERGE | 21 FAIL
```

After (consistent across all 4 profiles — fortress, balanced, performance, ludicrous):
```
77/86 PASS | 4 XFAIL | 0 DIVERGE | 5 FAIL
```

That's 90% pass rate with zero unexplained divergences.

## Remaining 5 FAIL

| Test | Issue |
|---|---|
| epoll_edge | EPOLLET (edge-triggered) doesn't suppress re-fire |
| alarm_delivery | Signal handler not invoked when waking from pause() |
| sigsuspend_wake | Signal handler not invoked during sigsuspend |
| execve_reset | Signal disposition not properly reset across execve |
| mmap_shared | MAP_SHARED writes not visible across fork |

## 4 XFAIL (known limitations)

| Test | Reason |
|---|---|
| epoll_level | epoll_wait blocking path hangs (timeout>0) |
| mprotect_roundtrip | SIGSEGV from page fault not delivered to userspace handler |
| munmap_partial | SIGSEGV kills process instead of invoking registered handler |
| ns_uts | Linux test runner lacks CAP_SYS_ADMIN; Kevlar doesn't enforce caps yet |

## Takeaway

Writing the tests was fast (~3 hours for 55 tests). Running them found 21 real
bugs in under 5 minutes; 19 were fixed in the same session, raising pass rate
from 55% (47/86) to 90% (77/86). The Arc downcast bug alone affected rename and
hard link — two operations that would silently corrupt any package manager.
Contract tests pay for themselves immediately.
