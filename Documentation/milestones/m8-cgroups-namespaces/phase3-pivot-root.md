# Phase 3: pivot_root and Filesystem Isolation

**Duration:** 2-3 days
**Prerequisite:** Phase 2 (namespaces)
**Goal:** Implement `pivot_root(2)` syscall and `/proc/[pid]/mountinfo`. Add MS_PRIVATE mount propagation. These are required by container runtimes and systemd to switch root filesystem within a mount namespace.

## pivot_root Semantics

`pivot_root(new_root, put_old)`:
1. Both must be directories; `put_old` must be at or under `new_root`.
2. `new_root` must be a mount point (not the current root).
3. Old root gets moved to `put_old`.
4. Process root and cwd updated accordingly.
5. Caller typically follows with `umount2(put_old, MNT_DETACH)`.

## Files to Create

1. **`kernel/syscalls/pivot_root.rs`** — `sys_pivot_root(new_root, put_old)`.

## Files to Modify

1. **`kernel/syscalls/mod.rs`** — Add `mod pivot_root;`. Syscall numbers: SYS_PIVOT_ROOT=155 (x86_64), 41 (ARM64). Add dispatch arm.

2. **`kernel/fs/mount.rs`** — Add `MountPropagation` enum (Private, Shared). Add `mount_id` counter. Add `pivot_root()` method to RootFs. Extend MountEntry with mount_id, parent_id, source, options. Add `format_mountinfo()` for /proc output.

3. **`kernel/fs/procfs/proc_self.rs`** — Add `"mountinfo"` to ProcPidDir::lookup(). ProcPidMountinfo reads from process's mount namespace. Add to readdir entries.

4. **`kernel/syscalls/mount.rs`** — Handle `MS_PRIVATE = 1 << 18` flag. Mark mount as private in namespace's mount table.

5. **`kernel/namespace/mnt.rs`** — Add `pivot_root()` method to MountNamespace.

## /proc/[pid]/mountinfo Format

```
<mount_id> <parent_id> <major>:<minor> <root> <mount_point> <options> <optional> - <fstype> <source> <super_options>
```

Example:
```
1 0 0:1 / / rw - initramfs none rw
2 1 0:2 / /proc rw - proc none rw
3 1 0:3 / /sys rw - sysfs none rw
4 1 0:4 / /dev rw - devtmpfs none rw
5 1 0:5 / /sys/fs/cgroup rw - cgroup2 none rw
```

## Contract Test

**`testing/contracts/subsystems/mountinfo.c`**
1. Open `/proc/self/mountinfo`, read contents.
2. Verify at least one line exists.
3. Verify format: line contains ` - ` separator.
4. Verify `proc` appears as a filesystem type.

## Success Criteria

- [ ] `pivot_root()` returns 0 on valid arguments
- [ ] After pivot_root, root filesystem is swapped
- [ ] `/proc/self/mountinfo` readable with correct format
- [ ] `mount(..., MS_PRIVATE)` marks mount as private
- [ ] `pivot_root` returns EINVAL for invalid arguments
- [ ] All existing contract tests pass
