# Blog 109: "Hello from Kevlar!" — GCC full pipeline works end-to-end

**Date:** 2026-03-22
**Milestone:** M10 Alpine Linux

## The Milestone

User-compiled C programs run on Kevlar for the first time:

```
/ # echo '#include <stdio.h>
int main(){printf("Hello from Kevlar!\n");return 0;}' > hello.c
/ # gcc -o hello hello.c
/ # ./hello
Hello from Kevlar!
```

Three test programs verified:
1. **Minimal return-42**: compiles, runs, exits with code 42 ✓
2. **Hello world with printf**: compiles, prints output, exits 0 ✓
3. **Fibonacci with -O2**: compiles with optimization, fib(10)==55 ✓

---

## Bug Fix: CLONE_FILES fd table independence

**Symptom:** OpenRC's `posix_spawn` crashed with EBADF when reading
the exec-success pipe. The crash report showed:

```
pipe2([5,6], O_CLOEXEC)   ← posix_spawn pipe
clone(0x4111)              ← CLONE_VM|CLONE_VFORK
close(6)                   ← parent closes write end
read(5) → -9 (EBADF)      ← pipe destroyed!
```

**Root cause:** `clone(CLONE_VM)` without `CLONE_FILES` should give
the child an independent fd table copy. We were sharing the fd table
via `Arc::clone`. When the child did `execve`, CLOEXEC closed ALL
pipe fds in the SHARED table, destroying the parent's pipe.

**Fix:** Non-CLONE_THREAD children get an independent fd table copy
(same pattern as `fork()`). CLONE_THREAD children (pthreads) still
share the fd table.

```rust
opened_files: if is_thread {
    Arc::clone(&parent.opened_files)  // threads share
} else {
    Arc::new(SpinLock::new(parent.opened_files.lock_no_irq().clone()))
},
```

---

## Session Summary (2026-03-22)

### Bugs Fixed (11 commits)
1. **brk PIE heap limit** — brk rejected all PIE heap expansions
2. **valloc VMA skip** — mmap returned addresses overlapping heap
3. **CLONE_VFORK blocking** — posix_spawn parent didn't block
4. **__WCLONE in wait4** — posix_spawn pipe signaling
5. **RTM_SETLINK netlink** — BusyBox ip link set
6. **ext4 extent atomicity** — directory entry visibility race
7. **AT_PHDR for non-PIE** — gcc binary crashed on load
8. **clone children.push** — wait4 returned ECHILD
9. **tgid for non-CLONE_THREAD** — exit_group killed gcc
10. **CLONE_FILES independence** — exec destroyed parent's pipe fds
11. **fchownat dirfd + 151 contract tests**

### What Works on Alpine/Kevlar
- GCC 14.2.0 compiles AND runs C programs
- apk update/add — 25,397 packages
- curl HTTP downloads
- Alpine boots to interactive shell
- OpenRC starts (crashes in deptree, non-fatal)
- 151 contract tests pass
