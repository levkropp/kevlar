# Phase 2: /proc File Expansion

**Duration:** ~3 days
**Prerequisite:** Phase 1
**Goal:** Implement the 8-10 most-read /proc files that real tools depend on.

## Scope

Add implementations for:
- `/proc/[pid]/maps` — memory layout (critical for malloc, strace, debuggers)
- `/proc/[pid]/stat` — process stats (needed by ps, top, systemd)
- `/proc/[pid]/status` — human-readable process info
- `/proc/[pid]/cmdline` — command line arguments
- `/proc/[pid]/fd/` — directory of open file descriptors
- `/proc/[pid]/fd/[fd]` — symlink to file path
- `/proc/meminfo` — memory usage
- `/proc/mounts` — mounted filesystems (stub: just root)
- `/proc/version` — kernel version string
- `/proc/sys/` — sysctl stubs (world-readable, always return defaults)

## Implementation Details

### /proc/[pid]/maps

Format:
```
7fff8000-7fffa000 r-xp 00000000 00:01 <inode>             /lib/ld-musl-x86_64.so.1
...
```

Implementation:
- Iterate over `Process.vm().vmas()`
- For each VMA, format: `[start]-[end] [perms] [offset] [dev] [inode] [path]`
- Permissions: r=PROT_READ, w=PROT_WRITE, x=PROT_EXEC, p=private/s=shared
- Path: lookup inode path from File/VMA, or "<anonymous>" for anon/heap/stack
- No allocation — write to user buffer via iterator

### /proc/[pid]/stat

Format (space-separated):
```
pid (comm) state ppid pgrp session tty_nr tpgid flags minflt cminflt majflt cmajflt utime stime ...
1 (init) S 0 1 1 0 -1 4194304 1234 0 5678 0 123 45 20 0 0 0 0 0 0 0 0 0 0 0 0 0 0
```

Implementation:
- Read from Process struct: pid, comm, state, ppid, pgrp, tgid (session)
- Read from ProcessState for utime/stime (might need to add)
- Most fields can be dummy values (0) initially
- Used by ps, top for sorting and display

### /proc/[pid]/status

Human-readable format:
```
Name:	init
Pid:	1
PPid:	0
...
VmPeak:	12345 kB
VmSize:	12345 kB
VmRSS:	1234 kB
```

Implementation:
- Read from Process struct
- For memory: sum all VMA sizes (VmSize), count resident pages (VmRSS)

### /proc/[pid]/cmdline

Format: null-separated argv, or "(swapper)" if kernel thread
Implementation: store `argv: Vec<Vec<u8>>` in Process at exec time

### /proc/[pid]/fd/

Directory listing of open file descriptors.
For each FD in `opened_files`:
```
readdir() → DirEntry(name=format!("{}", fd), ino=(pid_ino | (fd << 4)))
readlink(ino) → resolve to file path (requires FileLike trait method)
```

### /proc/meminfo

```
MemTotal:       1048576 kB
MemFree:        512345 kB
MemAvailable:   512000 kB
...
```

Implementation: Call `page_allocator::stats()` or similar to get free/used pages

### /proc/mounts

```
/dev/root / ext4 rw,relatime 0 0
tmpfs /tmp tmpfs rw,relatime 0 0
procfs /proc proc rw,relatime 0 0
```

Implementation: Return hardcoded entries (Phase 3 will be dynamic)

### /proc/version

```
Kevlar 0.1.0 (x86_64)
```

### /proc/sys/ (Stub)

Any file under `/proc/sys/` should return sensible defaults:
- `/proc/sys/kernel/ostype` → "Kevlar"
- `/proc/sys/kernel/osrelease` → "0.1.0"
- `/proc/sys/kernel/version` → "#1 SMP Mon Mar 13 00:00:00 UTC 2026"

## Key Challenges

1. **No allocations in read paths:** All formatting must be streaming (write to
   user buffer directly). Use format macros carefully.
2. **File descriptor path lookup:** FileLike needs a method to return its path.
   For regular files, store in Inode. For sockets/pipes, return a placeholder.
3. **Memory accounting:** Need to count resident pages accurately (currently
   all pages are demand-paged, so ~all mapped = resident for now).
4. **Signal mask format:** /proc/[pid]/status shows signal masks as hex. Ensure
   compat with what sigprocmask reports.

## Testing

- `cat /proc/self/maps` should show libc + binary + heap + stack
- `cat /proc/self/stat` should parse correctly (fields separated by spaces)
- `ps aux` should work (reads /proc/*/stat)
- `top` should work (reads /proc/*/stat repeatedly)
- `lsof` should work (reads /proc/*/fd/)
- `/proc/sys/kernel/ostype` should return "Kevlar"

## Integration Points

- **Process:** Add `argv: Vec<Vec<u8>>` and `exe_path: String` fields
- **FileLike trait:** Add `fn path(&self) -> Option<PathBuf>` method
- **VirtualMemory:** Add method to count resident pages
