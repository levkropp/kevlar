# M9.8 Phase 3: procfs Additions

## Overview

Add missing `/proc/sys/` entries that systemd reads during early boot. All
changes are in `kernel/fs/procfs/mod.rs` within the `ProcFs::new()` constructor.

## 3.1 — kptr_restrict + dmesg_restrict

**Location:** In the existing `sys_kernel_dir` block

systemd reads these files to determine kernel security settings. Missing files
cause warning messages in the boot log.

```rust
sys_kernel_dir.add_file("kptr_restrict",  Arc::new(ProcSysStaticFile("1\n")) as Arc<dyn FileLike>);
sys_kernel_dir.add_file("dmesg_restrict", Arc::new(ProcSysStaticFile("0\n")) as Arc<dyn FileLike>);
```

- `kptr_restrict=1`: Hide kernel pointers from unprivileged users (safe default)
- `dmesg_restrict=0`: Allow dmesg access (needed for systemd-journald)

## 3.2 — /proc/sys/vm/ Subdirectory

**Location:** After the `sys_kernel_dir` block

systemd and glibc check `/proc/sys/vm/overcommit_memory` and
`/proc/sys/vm/max_map_count` during boot and service startup.

```rust
let sys_vm_dir = sys_dir.add_dir("vm");
sys_vm_dir.add_file("overcommit_memory", Arc::new(ProcSysStaticFile("0\n")) as Arc<dyn FileLike>);
sys_vm_dir.add_file("max_map_count",     Arc::new(ProcSysStaticFile("65530\n")) as Arc<dyn FileLike>);
```

- `overcommit_memory=0`: Heuristic overcommit (Linux default)
- `max_map_count=65530`: Default maximum number of memory map areas

## Verification

```bash
make check                             # type-check
make RELEASE=1 test-systemd-v3        # no ENOENT warnings for these paths
```
