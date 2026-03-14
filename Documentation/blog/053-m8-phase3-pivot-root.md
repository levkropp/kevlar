# M8 Phase 3: pivot_root and Filesystem Isolation

Phase 3 adds the `pivot_root(2)` syscall, `/proc/[pid]/mountinfo`,
and `MS_PRIVATE` mount flag support.

## /proc/[pid]/mountinfo

The mountinfo file provides detailed mount information in the Linux
standard format:

```
mount_id parent_id major:minor root mount_point options - fstype source super_options
```

The MountTable now tracks mount IDs and parent relationships.
`format_mountinfo()` generates the content for any process's
/proc/[pid]/mountinfo.

## pivot_root(2)

Stub implementation that validates arguments (new_root must be a
directory) and returns success.  This lets systemd proceed through
its early boot sequence.  Full root-swapping semantics will be
fleshed out when we have real container workloads that need it.

## MS_PRIVATE

`mount()` now handles `MS_PRIVATE` and `MS_REC` flags.  These are
flag-only calls (no filesystem type) that mark mounts as private
to prevent mount event propagation between namespaces.  Accepted
silently since we don't propagate mounts yet.

## Results

- 28/29 PASS, 1 XFAIL (ns_uts: needs root on Linux)
- New mountinfo contract test passes on both Linux and Kevlar
