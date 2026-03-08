# M1: Static Busybox — Implementation Plan

**Status: COMPLETE** — BusyBox boots, interactive shell works (echo, ls, cat verified).

21 new syscalls + 5 bugfixes across 7 phases, ~880 lines total.
Plus: dependency upgrades (smoltcp 0.7→0.12, etc.), target spec modernization,
two critical boot bug fixes (EFER.NXE, compiler-builtins-mem SSE crash).

## Phase 0: Infrastructure (do first)

### 0A. Add `prot` field to VmArea + NX bit support (~80 lines, HIGH risk)
- Add `prot: MMapProt` field to `VmArea` in `kernel/mm/vm.rs`
- Thread through `add_vm_area()`, `Vm::new()` (stack=RW, heap=RW)
- Thread through `page_fault::handle_page_fault()` where `map_user_page` currently ignores protection
- Add `PageAttrs::NO_EXECUTE` flag (bit 63 on x86-64) to `runtime/x64/paging.rs`
- Add `map_user_page_with_flags()` that translates MMapProt → PageAttrs
- Current `map_user_page` always sets WRITABLE — must change to respect prot

### 0B. Add directory mutation methods (~60 lines, low risk) ✅ DONE
- Added `unlink()`, `rmdir()`, `rename()` to `Directory` trait in `kernel/fs/inode.rs` (default: ENOSYS)
- Implemented on `tmpfs::Dir` with cross-directory rename using pointer-ordered locking
- Added `ENOTEMPTY` to Errno enum
- Reference: OSv fs/vfs/vfs_syscalls.cc (BSD-3-Clause)

### 0C. Add `umask` field to Process (~15 lines, low risk) ✅ DONE
- Added `umask: AtomicCell<u32>` to `Process`, default `0o022`, propagated through `fork()`
- Added `umask()` and `set_umask()` accessor methods

## Phase 1: Trivial syscalls (~40 lines, very low risk) ✅ DONE

All under 10 lines each. New files in `kernel/syscalls/` + dispatch entries in `mod.rs`.

- [x] `getegid`(108) — return 0
- [x] `getgid`(104) — return 0 (bonus, not originally planned)
- [x] `getpgrp`(111) — reads process_group().lock().pgid()
- [x] `sched_yield`(24) — call `process::switch()`, return 0
- [x] `umask`(95) — read/write `process.umask`, return old value
- [x] `dup`(32) — call `opened_files.dup(fd, None, OpenOptions::empty())`
- [x] `vfork`(58) — identical to `sys_fork()` initially

## Phase 2: FD plumbing (~50 lines, low risk) ✅ DONE

- [x] `dup3`(292) — like dup2 + O_CLOEXEC flag, EINVAL if old==new
- [x] `pipe2`(293) — like pipe + flags O_CLOEXEC/O_NONBLOCK
- [x] Fix `fcntl`(72) — added F_GETFD (return close_on_exec flag) and F_GETFL (return O_NONBLOCK etc)

## Phase 3: The *at syscalls + file ops (~120 lines, medium risk) ✅ DONE

**Critical path — musl uses openat/newfstatat exclusively.**

- [x] `openat`(257) — parse dirfd, pass as `CwdOrFd` to existing `lookup_path_at`
  - Reference: OSv fs/vfs/vfs_syscalls.cc sys_open (BSD-3-Clause)
- [x] `newfstatat`(262) — like stat + dirfd + AT_EMPTY_PATH + AT_SYMLINK_NOFOLLOW
  - Reference: OSv fs/vfs/vfs_syscalls.cc sys_fstatat (BSD-3-Clause)
- [x] `access`(21) — resolve path, return 0 or ENOENT (no real UID tracking)
  - Reference: OSv fs/vfs/vfs_syscalls.cc sys_access (BSD-3-Clause)
- [x] `lseek`(8) — added `set_pos()` to OpenedFile, handles SEEK_SET/CUR/END
  - Reference: OSv fs/vfs/vfs_syscalls.cc sys_lseek (BSD-3-Clause)
  - Also fixed `tmpfs::File::stat()` to return actual `data.len()` as file size

## Phase 4: Filesystem mutations (~70 lines, medium risk) ✅ DONE

Depends on Phase 0B.

- [x] `unlink`(87) — resolve parent dir, call `dir.unlink(name)`, EISDIR if target is dir
  - Reference: OSv fs/vfs/vfs_syscalls.cc sys_unlink (BSD-3-Clause)
- [x] `rmdir`(84) — resolve parent dir, call `dir.rmdir(name)`, ENOTEMPTY check
  - Reference: OSv fs/vfs/vfs_syscalls.cc sys_rmdir (BSD-3-Clause)
- [x] `rename`(82) — resolve both parents, call `old_parent.rename()`
  - Reference: OSv fs/vfs/vfs_syscalls.cc sys_rename (BSD-3-Clause)
  - Cross-directory rename uses pointer-ordered locking to prevent deadlock

## Phase 5: Time & system info (~115 lines, low risk) ✅ DONE

- [x] `nanosleep`(35) — parse timespec, compute ms, call existing `_sleep_ms`
  - Reference: OSv core/osv_clock.cc (BSD-3-Clause)
- [x] `gettimeofday`(96) — reads wall clock, returns struct timeval
  - Reference: OSv core/osv_clock.cc (BSD-3-Clause)
- [x] `sysinfo`(99) — uptime from monotonic clock, totalram/freeram from page allocator, procs count
  - Added `process_count()` helper to process module
- [x] `getrlimit`(97) — return fake limits: RLIM_INFINITY for most, 8MB stack, 1024 NOFILE
  - Reference: OSv core/rlimit.cc (BSD-3-Clause)

## Phase 6: Memory management (~280 lines, HIGH risk) ✅ DONE

Depends on Phase 0A. Reference: OSv core/mmu.cc (BSD-3-Clause) for VMA tracking logic.

### 0A. Infrastructure ✅
- Added `prot: MMapProt` field to `VmArea`
- Added `NO_EXECUTE` (bit 63) to `PageAttrs` in `runtime/x64/paging.rs`
- Added `map_user_page_with_prot()` — translates PROT_READ/WRITE/EXEC → PageAttrs
- Added `update_page_flags()` — updates PTE flags on mapped pages
- Added `unmap_user_page()` — clears PTE and returns physical address
- Added `flush_tlb()` (invlpg) and `flush_tlb_all()` (CR3 reload)
- Updated page fault handler to respect VMA prot flags

### Syscalls ✅
- [x] Fix `mmap`(9) — stores prot in VmArea via `add_vm_area_with_prot()`
- [x] `mprotect`(10) — validates alignment, splits VMAs at boundaries via `update_prot_range()`,
  walks page table updating PTE flags, flushes TLB per-page
- [x] `munmap`(11) — validates alignment, removes/splits VMAs via `remove_vma_range()`,
  walks page table clearing PTEs, frees physical pages, flushes TLB

## Phase 7: Bugfixes (~50 lines, low risk) ✅ DONE

- [x] Fix `wait4`(61) — status encoding: `(exit_code << 8)` for normal exit
  - Also handles pid == -1 (wait for any child)
- [x] Fix `select`(23) — handle errorfds (POLLERR|POLLHUP), fix writefds checking POLLIN→POLLOUT
- [x] Fix `getsockopt`(55) — handle SOL_SOCKET/SO_ERROR (return 0) and SO_TYPE (return SOCK_STREAM)

## Dependency Graph

```
Phases 1, 2, 3, 5, 7 — all independent ✅ ALL DONE
Phase 4 — depends on 0B (Directory trait) ✅ DONE
Phase 6 — depends on 0A (VmArea prot field) ✅ DONE

ALL PHASES COMPLETE.
```

## Key Files Modified

- `kernel/fs/inode.rs` — Directory trait (unlink/rmdir/rename)
- `kernel/fs/tmpfs.rs` — tmpfs impl of mutations, fix stat file size
- `kernel/fs/opened_file.rs` — added `set_pos()` for lseek
- `kernel/mm/vm.rs` — `prot` field on VmArea, `add_vm_area_with_prot()`, VMA splitting (remove_vma_range, update_prot_range)
- `kernel/mm/page_fault.rs` — prot-aware page mapping
- `kernel/process/process.rs` — umask field, `process_count()` helper
- `kernel/process/mod.rs` — re-export `process_count`
- `kernel/result.rs` — added `ENOTEMPTY` errno
- `kernel/syscalls/mod.rs` — dispatch table (24 new entries)
- `kernel/syscalls/mmap.rs` — stores prot in VmArea
- `kernel/syscalls/fcntl.rs` — F_GETFD, F_GETFL
- `kernel/syscalls/wait4.rs` — status encoding fix + pid==-1
- `kernel/syscalls/select.rs` — writefds/errorfds fix
- `kernel/syscalls/getsockopt.rs` — SO_ERROR/SO_TYPE handling
- `runtime/x64/paging.rs` — NO_EXECUTE bit, map_user_page_with_prot, update_page_flags, unmap_user_page, flush_tlb

## New Files Created

- `kernel/syscalls/{getegid,getpgrp,sched_yield,umask,dup,vfork}.rs` — Phase 1
- `kernel/syscalls/{dup3,pipe2}.rs` — Phase 2
- `kernel/syscalls/{openat,newfstatat,access,lseek}.rs` — Phase 3
- `kernel/syscalls/{unlink,rmdir,rename}.rs` — Phase 4
- `kernel/syscalls/{nanosleep,gettimeofday,sysinfo,getrlimit}.rs` — Phase 5
- `kernel/syscalls/{mprotect,munmap}.rs` — Phase 6

## Testing

Each phase gets verified by running the existing test suite (`make check`, integration tests).
Final validation: boot a static musl BusyBox and run `ls -la /`.
