# Phase 1: /proc Filesystem Foundation

**Duration:** ~3 days
**Blocker for:** Phase 2, Phase 4
**Goal:** Implement basic /proc VFS service and make it mountable.

## Scope

Implement a read-only procfs mount point with the foundational inode/dentry
model. Initially support only:
- `/proc/self` (symlink to `/proc/[pid]`)
- `/proc/[pid]/` (directory, lists open via readdir)
- `/proc/cpuinfo` (static file, lists CPU count)

Additional /proc files are Phase 2.

## Implementation Plan

### 1. Procfs Service Crate

Create `services/kevlar_procfs/src/lib.rs`:
```rust
pub struct ProcFs { /* ... */ }

impl FilesystemService for ProcFs {
    fn mount(&mut self, path: &Path) -> Result<()> { /* ... */ }
    fn lookup(&self, ino: Ino, name: &str) -> Result<Ino> { /* ... */ }
    fn read(&self, ino: Ino, offset: u64, buf: &mut [u8]) -> Result<usize> { /* ... */ }
    fn readdir(&self, ino: Ino) -> Result<Vec<DirEntry>> { /* ... */ }
}
```

- No allocation for file data (computed on-read)
- /proc inode numbers: stable mapping from (pid, file_kind) tuples
- /proc root (ino=1) is fixed; /proc/[pid]/ inodes are (pid << 16) | kind

### 2. Magic Symlink for /proc/self

- At mount time, create a special inode (kind=SELF_SYMLINK)
- When readlink() is called on ino=SELF_SYMLINK, return current process's pid
- This requires access to `current_process()` from within procfs

### 3. /proc/cpuinfo Static File

Simple file that reads `arch::num_online_cpus()` and formats:
```
processor	: 0
vendor_id	: GenuineKevlar
cpu family	: 6
model		: 158
...
processor	: 1
...
```

No allocation — write directly to user buffer via a formatting iterator.

### 4. /proc/[pid]/ Directory Listing

In `readdir()`, iterate over `PROCESSES` to enumerate live pids.
For each pid, return synthetic DirEntries:
- `.` (ino = pid-dir)
- `..` (ino = root)
- `exe` (ino = pid-dir | EXE_FILE)
- `cmdline` (ino = pid-dir | CMDLINE_FILE)
- `maps` (ino = pid-dir | MAPS_FILE)
- ... (more in Phase 2)

## Key Decisions

**No pre-allocated dentries:** Unlike traditional filesystems, /proc's directory
structure is dynamic. A process's `/proc/[pid]/` inode only exists if the
process is alive. Use on-demand inode creation during readdir().

**Inode numbering scheme:** Stable but derived:
- Root: ino = 1
- /proc/self: ino = 2
- /proc/cpuinfo: ino = 3
- /proc/[pid]/: ino = (pid.as_u32() << 16) | 0 (dir), with sub-inodes:
  - | 1 = exe
  - | 2 = cmdline
  - | 3 = maps
  - | 4 = stat
  - | 5 = fd (directory)
  - ... (more in Phase 2)

**Permission model:** `/proc/self/*` and `/proc/[pid]/*` are world-readable if
the process is uid=0 (init) or readable only by owner otherwise. Implement via
`can_read()` checks that call `current_process().uid()`.

## Testing

- Mount /proc and verify `/proc/self` exists
- Read `/proc/self/` and check it resolves to `/proc/1/`
- Read `/proc/cpuinfo` and verify it lists N processors (for -smp N)
- Read `/proc/1/` and verify it lists exe, cmdline, maps, etc.
- Verify permission checks: /proc/[other_pid]/ should fail for non-owner

## Integration Points

- **VFS:** Modify `kernel/vfs/mod.rs` to register procfs as a built-in mount
- **Process:** Add `exe_path: String` field to Process for /proc/[pid]/exe symlink
- **Architecture:** Call `arch::num_online_cpus()` from procfs for cpuinfo
