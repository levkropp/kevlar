# Blog 108: GCC compiles, links, and produces binaries on Alpine/Kevlar

**Date:** 2026-03-22
**Milestone:** M10 Alpine Linux

## The Milestone

GCC 14.2.0 compiles, assembles, and links C programs on Alpine/Kevlar:

```
/ # echo 'int main(){return 42;}' > /root/t.c
/ # gcc -o /root/t /root/t.c
/ # ls -la /root/t
-rw-r--r--    1 root     root         18272 Jan  1  1970 /root/t
```

The full pipeline runs: `cc1` → `as` → `collect2`/`ld` → 18KB ELF binary.

---

## The Investigation

### Symptom
gcc exited 0 but produced no output binary. The `-v` flag showed cc1
ran but `as` and `collect2` were never invoked. No error messages.

### Phase 1: Where does gcc stop?

Process tracing (`debug=process`) revealed gcc only spawned cc1 — no
`as` or `collect2`. The process event log showed:

```
process_fork: parent=3(gcc), child=4
process_exec: pid=4, argv0="cc1"
process_exit: pid=4, status=0
```

No PID 5 (as) or PID 6 (collect2) ever appeared.

### Phase 2: posix_spawn protocol

gcc uses musl's `posix_spawn` which calls `clone(0x4111)`:
- `CLONE_VM` (0x100) — share address space
- `CLONE_VFORK` (0x4000) — parent blocks until child execs
- `SIGCHLD` (0x11) — notify parent on exit

The protocol: parent creates a pipe, clones, child execs cc1. The
pipe's CLOEXEC write end closes on exec, signaling success to the
parent. Parent reads pipe → 0 bytes → exec succeeded.

### Phase 3: CLONE_VFORK deadlock

Syscall tracing showed gcc's `clone` syscall entry but **no exit** —
gcc was permanently blocked. Adding traces to the VFORK wait loop:

```
clone_vfork: pid=3 child=4 done_already=false
clone_vfork: loop 1 sleeping
wake_vfork: child=4 parent=3 waiters=1
```

The wake fired! `wake_all` dequeued gcc (waiters=1→0). But gcc
never woke from `sleep_signalable_until`.

### Phase 4: resume() early return

Tracing `resume()` revealed the smoking gun:

```
resume(3): old_state=ExitedWith(0)
```

**gcc's state was `ExitedWith(0)` — it had been killed while sleeping!**

### Phase 5: Root cause — exit_group kills parent

`new_thread()` (used for `clone(CLONE_VM)`) set `tgid: parent.tgid`,
putting cc1 in gcc's thread group. When cc1 called `exit_group(0)`,
the kernel killed all processes with the same tgid:

```rust
// exit_group() — kills all threads in the thread group
let siblings = table.values()
    .filter(|p| p.tgid == tgid && p.pid != current.pid)
    .collect();
for sibling in siblings {
    sibling.set_state(ProcessState::ExitedWith(status));
}
```

gcc (PID 3) had `tgid = 3`. cc1 (PID 4) also had `tgid = 3`. When
cc1 called `exit_group(0)`, it found gcc as a "sibling" and set it to
`ExitedWith(0)`. gcc was still sleeping in the VFORK wait queue. When
`wake_all` later called `resume(gcc)`, resume saw `ExitedWith` and
returned early without re-enqueuing gcc in the scheduler. gcc was gone.

## The Fix

One-line change in `new_thread()`:

```rust
// Before: always shared parent's thread group
tgid: parent.tgid,

// After: only share for CLONE_THREAD (actual threads)
tgid: if is_thread { parent.tgid } else { pid },
```

For `CLONE_THREAD` (pthreads): child shares parent's tgid — correct,
exit_group should kill all threads.

For `CLONE_VM|CLONE_VFORK` (posix_spawn): child gets its own tgid —
correct, exit_group only affects the child's own (empty) thread group.

## Other Fixes This Session

### valloc allocator VMA conflicts
`alloc_vaddr_range` was a bump allocator that didn't check for existing
VMAs. After `set_heap_bottom` placed the heap VMA in the valloc region,
mmap got addresses overlapping the heap → EINVAL → "sh: out of memory".

Fix: `alloc_vaddr_range` now loops and skips conflicting VMAs.

### ext4 alloc_extent_block atomicity
`alloc_extent_block` wrote the inode (with updated extent tree) BEFORE
the directory size was updated. A concurrent reader could see the extent
but calculate `num_blocks` from the old size, missing the new block.

Fix: removed premature `write_inode` from `alloc_extent_block`. The
caller writes the inode once after both extent tree AND size are set.

## Results

| Feature | Before | After |
|---------|--------|-------|
| `gcc -o hello hello.c` | silent exit 0, no binary | **compiles + links, 18KB binary** |
| `gcc --version` | works | works |
| Alpine boot | zero crashes | zero crashes |
| `sh: out of memory` | crash on exec | **fixed** |
| ext4 dir visibility | race condition | **atomic inode write** |

## What's Next

- Execute the compiled binary (`/root/t`)
- Run compiled "Hello from Kevlar!" program
- OpenRC boot improvements (ip/openrc sysinit errors)
- HTTPS support for apk repos
