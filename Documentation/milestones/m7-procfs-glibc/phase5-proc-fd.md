# Phase 5: /proc/[pid]/fd/ Directory

**Duration:** ~1 day
**Prerequisite:** Phase 1
**Goal:** Implement /proc/[pid]/fd/ directory listing and fd symlinks.

## Scope

`/proc/[pid]/fd/` is a directory where each entry is a symlink named
by file descriptor number, pointing to the opened file's path.

```
/proc/self/fd/0 -> /dev/console
/proc/self/fd/1 -> /dev/console
/proc/self/fd/2 -> /dev/console
/proc/self/fd/3 -> pipe:[12345]
```

## Implementation

### Directory listing

`readdir("/proc/[pid]/fd/")` returns one entry per open fd:
- Name: fd number as decimal string ("0", "1", "2", ...)
- Type: DT_LNK (symlink)

```rust
fn readdir_fd(pid: PId) -> Vec<DirEntry> {
    let process = find_process(pid)?;
    let files = process.opened_files_no_irq();
    files.iter_fds().map(|fd| {
        DirEntry {
            name: format!("{}", fd.as_usize()),
            ino: (pid << 16) | (0x100 + fd.as_usize()),
            type_: DT_LNK,
        }
    }).collect()
}
```

### Symlink resolution

`readlink("/proc/[pid]/fd/3")` returns the file path:
- Regular files: `/path/to/file`
- Pipes: `pipe:[inode]`
- Sockets: `socket:[inode]`
- Special files: `/dev/null`, `/dev/tty`
- Anonymous: `anon_inode:[type]`

Requires `FileLike` trait extension:

```rust
pub trait FileLike {
    // ... existing methods ...

    /// Return a display path for /proc/[pid]/fd/ symlinks.
    fn display_path(&self) -> &str {
        "(unknown)"
    }
}
```

Override in each FileLike implementation:
- `NullFile::display_path() -> "/dev/null"`
- `ZeroFile::display_path() -> "/dev/zero"`
- `Tty::display_path() -> "/dev/tty"`
- `OpenedFile::display_path() -> self.path.as_str()`
- `Pipe::display_path() -> "pipe:[N]"`

### OpenedFiles accessor

Need `pub fn iter_fds(&self) -> impl Iterator<Item = Fd>` on the
opened files table.

## Testing

Contract test: `testing/contracts/subsystems/proc_fd.c`
```c
// Open /dev/null, read /proc/self/fd/ directory
// Verify fd 0, 1, 2 exist (stdin/stdout/stderr)
// readlink on the opened fd returns "/dev/null"
```

## Success criteria

- [ ] `ls /proc/self/fd/` shows open file descriptors
- [ ] `readlink /proc/self/fd/0` returns correct path
- [ ] `lsof` can enumerate open files (basic support)
