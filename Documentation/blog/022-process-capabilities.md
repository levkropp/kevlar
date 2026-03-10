# Blog 022: Process Management & Capabilities

**M4 Phase 5 — prctl, capget/capset, UID/GID tracking, subreaper reparenting**

systemd is a process manager. It needs to name its threads, mark itself as a
subreaper, check capabilities, and track UIDs. Phase 5 adds all of this.

## UID/GID Tracking

Previously every `getuid`/`getgid` returned hardcoded 0. Now the Process struct
has real fields:

```rust
uid: AtomicU32,
euid: AtomicU32,
gid: AtomicU32,
egid: AtomicU32,
```

`fork()` copies parent values to child. `setuid`/`setgid` store the values.
No permission checks yet — we're running everything as root — but the tracking
is faithful enough for systemd's credential logic to work.

## prctl(2)

systemd uses several prctl commands at startup:

| Command | Behavior |
|---------|----------|
| PR_SET_NAME | Set thread name (max 15 bytes), stored in `comm` field |
| PR_GET_NAME | Read thread name, falls back to argv0 |
| PR_SET_CHILD_SUBREAPER | Mark process as subreaper for orphan reparenting |
| PR_GET_CHILD_SUBREAPER | Query subreaper status |
| PR_SET_PDEATHSIG | Stub (accepted silently) |
| PR_GET_SECUREBITS | Returns 0 (no secure bits) |

The `comm` field is `SpinLock<Option<Vec<u8>>>` — `None` means "use argv0",
`Some(bytes)` is the explicitly set name. This shows up in `/proc/[pid]/comm`.

## Subreaper Reparenting

The key architectural piece. When a process exits, its children become orphans.
Linux normally reparents them to init (PID 1). With `PR_SET_CHILD_SUBREAPER`,
systemd can intercept this — orphaned children of systemd's subtree get
reparented to systemd instead.

```rust
fn find_subreaper_or_init(exiting: &Process) -> Arc<Process> {
    let mut ancestor = exiting.parent.upgrade();
    while let Some(p) = ancestor {
        if p.is_child_subreaper() {
            return p;
        }
        ancestor = p.parent.upgrade();
    }
    // Fall back to init (PID 1)
    PROCESSES.lock().get(&PId::new(1)).unwrap().clone()
}
```

This walks up the parent chain looking for the nearest subreaper. The reparented
children are moved to the new parent's children list, and `JOIN_WAIT_QUEUE` is
woken so `wait()` can see them.

## Linux Capabilities (Stub)

systemd checks capabilities with `capget()` to decide what it's allowed to do.
Our stub returns all capabilities granted:

- Version 3 protocol (`0x20080522`)
- Two 32-bit sets, both `effective = 0xFFFFFFFF`, `permitted = 0xFFFFFFFF`
- `capset()` accepts silently

Real capability enforcement comes later with multi-user support.

## Syscall Summary

| Syscall | x86_64 | ARM64 |
|---------|--------|-------|
| prctl | 157 | 167 |
| capget | 125 | 90 |
| capset | 126 | 91 |

~270 lines across 5 files. The subreaper logic is the most architecturally
important addition — it's how systemd maintains its process hierarchy even when
intermediate launcher processes exit.
