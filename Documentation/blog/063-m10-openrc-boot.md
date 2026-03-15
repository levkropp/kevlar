# M10 Phase 3: OpenRC Boot — From Manual Init to a Real Service Manager

Phase 2 got BusyBox init running with hardcoded mount commands in
`/etc/inittab`. Phase 3 replaces that with Alpine's OpenRC service
manager — the first real service supervisor to run on Kevlar.

## What is OpenRC?

OpenRC is Alpine Linux's service manager. Unlike systemd, it is not a
daemon — it runs, starts services for a given runlevel, and exits.
BusyBox init remains PID 1 and invokes OpenRC via inittab:

```
::sysinit:/sbin/openrc sysinit
::sysinit:/sbin/openrc boot
::wait:/sbin/openrc default
::respawn:/sbin/getty -L 115200 ttyS0 vt100
::shutdown:/sbin/openrc shutdown
```

OpenRC processes each runlevel in order, starting services like devfs,
dmesg, hostname, and bootmisc. Each service is a shell script in
`/etc/init.d/` executed by `/sbin/openrc-run`.

## The musl ABI wall

The first attempt crashed immediately — every OpenRC process got SIGSEGV
after dynamic linking completed. Syscall tracing showed all libraries
loaded successfully, relocations applied, then instant crash at the
first instruction of `main()`.

The root cause: a musl libc version mismatch. Our initramfs shipped
musl 1.1.24 (from the Ubuntu 20.04 Docker base), but OpenRC was compiled
on Alpine 3.21 against musl 1.2.5. The musl 1.2 series changed `time_t`
from 32-bit to 64-bit and reworked internal TLS layout — a hard ABI break.

The fix: upgrade all Docker build stages from Ubuntu 20.04 to 24.04,
which ships musl 1.2.4 (ABI-compatible with Alpine's 1.2.5). This also
required:

- **BusyBox 1.36.1 -> 1.37.0** — the `tc` applet used CBQ kernel structs
  removed from newer `linux-libc-dev` headers
- **Adding `binutils`** to musl-only build stages — Ubuntu 24.04's
  `musl-tools` no longer transitively depends on the assembler
- **Pinning systemd v245 build to 20.04** — its `meson.build` uses
  operators removed in meson >= 1.0

## Real `mknod` (the critical path)

OpenRC's `devfs` service mounts a fresh tmpfs on `/dev` then calls
`mknod` to recreate device nodes. Our previous stub (`SYS_MKNOD => Ok(0)`)
returned success without creating anything, so `/dev/console` vanished
after the devfs service ran.

The implementation has three parts:

**Device registry** maps Linux major:minor numbers to kernel device objects:

```rust
pub fn lookup_device(major: u32, minor: u32) -> Option<Arc<dyn FileLike>> {
    match (major, minor) {
        (1, 3) => Some(NULL_FILE.clone()),          // /dev/null
        (1, 5) => Some(Arc::new(ZeroFile::new())),  // /dev/zero
        (4, 64) | (5, 0) | (5, 1) => Some(SERIAL_TTY.clone()),
        (5, 2) => Some(PTMX.clone()),               // /dev/ptmx
        // ...
    }
}
```

**DeviceNodeFile** stores mode + rdev and redirects through `open()`:

```rust
fn open(&self, _options: &OpenOptions) -> Result<Option<Arc<dyn FileLike>>> {
    match lookup_device(self.major(), self.minor()) {
        Some(dev) => Ok(Some(dev)),
        None => Ok(None),
    }
}
```

This leverages the existing `FileLike::open()` hook (already used for
ptmx) — when a DeviceNodeFile is opened, the VFS replaces it with the
real device transparently.

**sys_mknod** resolves the parent directory, creates a DeviceNodeFile,
and inserts it via `Directory::link()`. Also wired `SYS_MKNODAT` (259
on x86_64) since BusyBox may use the `*at` variant.

## Writable `/proc/sys/kernel/hostname`

OpenRC's hostname service writes the hostname by echoing to
`/proc/sys/kernel/hostname`. Previously writes were silently discarded.
Five lines to call `uts.set_hostname()`:

```rust
fn write(&self, _offset: usize, buf: UserBuffer<'_>, _options: &OpenOptions) -> Result<usize> {
    let mut data = [0u8; 64];
    let mut reader = UserBufReader::from(buf);
    let n = reader.read_bytes(&mut data)?;
    let len = if n > 0 && data[n - 1] == b'\n' { n - 1 } else { n };
    current_process().namespaces().uts.set_hostname(&data[..len])?;
    Ok(n)
}
```

## devtmpfs mount

OpenRC's devfs service calls `mount -t devtmpfs devtmpfs /dev`. The
previous handler returned `Ok(0)` without mounting anything. Changed to
actually mount our DEV_FS at the target, so pre-existing device nodes
(and newly mknod'd ones) appear.

## Bonus: fixing `getpid()` for threads

While running the full test suite after the Ubuntu 24.04 upgrade, the
`getpid_same` threading test failed. The test creates a pthread and
checks that `getpid()` returns the same PID from both threads.

The bug: `sys_getpid()` returned `ns_pid` (the process's own
namespace-local PID). For the thread group leader this equals the TGID,
but for threads it's the thread's TID. POSIX requires `getpid()` to
return the TGID for all threads in a group.

```rust
// Before: returned thread's own PID (wrong for threads)
Ok(current_process().ns_pid().as_i32() as isize)

// After: return TGID with fast path for non-threads
let tgid = current.tgid();
if current.pid() == tgid {
    return Ok(current.ns_pid().as_i32() as isize);  // fast path
}
// ... slow path: translate tgid through PID namespace
```

The fast path (group leader, root namespace) avoids the Arc clone for
namespace lookup, keeping getpid at 69ns — 0.75x Linux KVM.

## Benchmark pipeline

Also wired up `make bench-report` to show current numbers:

- `make bench-kvm` — Kevlar benchmarks, extracts to `/tmp/kevlar-bench-balanced.txt`
- `make bench-linux` — Linux KVM baseline, writes `/tmp/linux-bench-kvm.txt`
- `make bench-report` — comparison table

Current: **27/37 faster than Linux, 10 at parity, 0 regressions.**

## Result

```
OpenRC 0.55.1 is starting up Linux 4.0.0 (x86_64) [DOCKER]
 * Mounting /proc ... [ ok ]
 * Mounting /run ... [ ok ]
 * /run/openrc: creating directory
 * /run/openrc: correcting mode
 * Caching service dependencies ... [ ok ]

Kevlar (Alpine)  /dev/ttyS0

kevlar login:
```

## Files changed

| File | Change |
|------|--------|
| `testing/Dockerfile` | Ubuntu 20.04 -> 24.04, BusyBox 1.37.0, OpenRC stage, Alpine musl libs |
| `testing/etc/inittab` | OpenRC runlevel invocations |
| `kernel/fs/devfs/mod.rs` | Device registry + DeviceNodeFile |
| `kernel/syscalls/mknod.rs` | New: real mknod/mknodat |
| `kernel/syscalls/mod.rs` | Wire SYS_MKNOD + SYS_MKNODAT |
| `kernel/fs/procfs/mod.rs` | Writable /proc/sys/kernel/hostname |
| `kernel/syscalls/mount.rs` | devtmpfs mount -> real DEV_FS |
| `kernel/syscalls/getpid.rs` | Return TGID for threads |
| `libs/kevlar_vfs/src/stat.rs` | Added S_IFBLK constant |
| `Makefile` | bench-kvm output, bench-linux, bench-report targets |
| `tools/bench-linux.py` | New: Linux KVM benchmark runner |
