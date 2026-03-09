# Phase 5: Process Management & Capabilities

**Goal:** Implement prctl, stub Linux capabilities, and make UID/GID tracking
real. systemd uses these for privilege management and service isolation.

**Prerequisite:** None (independent of epoll/socket work).

## Syscalls

| Syscall | Number | Priority | Notes |
|---------|--------|----------|-------|
| `prctl` | 157 | Required | Process control operations |
| `capget` | 125 | Required | Query Linux capabilities |
| `capset` | 126 | Required | Set Linux capabilities |
| `memfd_create` | 319 | Nice-to-have | Anonymous memory files |

## prctl Operations

systemd uses these prctl commands:

| Command | Value | Priority | Behavior |
|---------|-------|----------|----------|
| `PR_SET_CHILD_SUBREAPER` | 36 | Required | Reparent orphans to this process instead of init |
| `PR_SET_NAME` | 15 | Required | Set thread name (shows in /proc/[pid]/comm) |
| `PR_GET_NAME` | 16 | Nice-to-have | Get thread name |
| `PR_SET_PDEATHSIG` | 1 | Nice-to-have | Signal on parent death |
| `PR_GET_SECUREBITS` | 27 | Stub | Return 0 |

### PR_SET_CHILD_SUBREAPER

This is architecturally important. When a child's parent exits, the child is
reparented to the nearest ancestor marked as a subreaper (instead of PID 1).
systemd sets this so that service processes become children of systemd even if
the original launcher exits.

```rust
// In Process struct:
is_child_subreaper: AtomicBool,

// In reparent logic (when parent exits):
fn find_new_parent(orphan: &Process) -> Arc<Process> {
    let mut ancestor = orphan.parent();
    while let Some(p) = ancestor {
        if p.is_child_subreaper.load(Ordering::Relaxed) {
            return p;
        }
        ancestor = p.parent();
    }
    // Fall back to init (PID 1)
    PROCESSES.lock().get(&PId::new(1)).unwrap().clone()
}
```

## Linux Capabilities

systemd checks capabilities to decide what it's allowed to do. For initial
M4, we can stub this: all processes have all capabilities (we're running as
root anyway).

```rust
fn sys_capget(header, data) -> Result<isize> {
    // Return all capabilities granted.
    // Version 2 header (0x20080522) with 2 u32 sets.
    let caps = CapData {
        effective: 0xFFFFFFFF,
        permitted: 0xFFFFFFFF,
        inheritable: 0,
    };
    // Write to userspace...
    Ok(0)
}

fn sys_capset(header, data) -> Result<isize> {
    // Accept silently — we don't enforce capabilities yet.
    Ok(0)
}
```

Real capability enforcement comes later when we have multi-user support.

## UID/GID Tracking

Currently all user/group syscalls return 0 (stub). For systemd we need
real tracking even though we don't enforce permission checks:

```rust
// In Process struct (replace stubs):
uid: AtomicU32,   // real UID
euid: AtomicU32,  // effective UID
gid: AtomicU32,   // real GID
egid: AtomicU32,  // effective GID
groups: SpinLock<Vec<u32>>,  // supplementary groups
```

setuid/setgid/setgroups store values. getuid/geteuid/etc. return them.
No permission checks yet — just faithful tracking so systemd's logic works.

## memfd_create

Creates an anonymous file backed by memory. systemd uses this for sealed
memory regions and fd-based IPC.

```rust
fn sys_memfd_create(name_ptr, flags) -> Result<isize> {
    // Create anonymous tmpfs file with the given name.
    // Supports MFD_CLOEXEC and MFD_ALLOW_SEALING (ignore sealing for now).
    let file = TmpfsFile::new_anonymous(name);
    let fd = current.opened_file_table().lock().open(file, flags)?;
    Ok(fd as isize)
}
```

## Files to Create/Modify

- `kernel/syscalls/prctl.rs` (NEW) — prctl dispatch
- `kernel/syscalls/capability.rs` (NEW) — capget/capset stubs
- `kernel/syscalls/memfd.rs` (NEW) — memfd_create
- `kernel/process/process.rs` — uid/gid/euid/egid fields, subreaper flag
- `kernel/process/process.rs` — reparent logic for subreaper
- `kernel/syscalls/mod.rs` — dispatch entries + fix uid/gid stubs

## Integration Test

```c
// Test: prctl + uid tracking
prctl(PR_SET_NAME, "test_proc", 0, 0, 0);
char name[16] = {0};
prctl(PR_GET_NAME, name, 0, 0, 0);
assert(strcmp(name, "test_proc") == 0);

// Test: subreaper
prctl(PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0);
if (fork() == 0) {
    if (fork() == 0) {
        // Grandchild: after parent exits, should be reparented to us
        sleep(1);
        assert(getppid() == grandparent_pid);
        _exit(0);
    }
    _exit(0);  // Middle process exits
}
wait(NULL);  // Reap middle process
wait(NULL);  // Reap grandchild (we're subreaper)

// Test: capabilities
struct __user_cap_header_struct hdr = { _LINUX_CAPABILITY_VERSION_3, 0 };
struct __user_cap_data_struct data[2];
capget(&hdr, data);
assert(data[0].effective == 0xFFFFFFFF);  // all caps

printf("TEST_PASS process_caps\n");
```

## Reference

- Linux man pages: prctl(2), capabilities(7), capget(2), memfd_create(2)
- FreeBSD: `sys/compat/linux/linux_misc.c` (prctl emulation)
- Linux: `kernel/sys.c` (prctl), `security/commoncap.c` (capabilities)

## Estimated Complexity

~400-500 lines. Mostly straightforward. The subreaper reparenting logic is the
only architecturally interesting part.
