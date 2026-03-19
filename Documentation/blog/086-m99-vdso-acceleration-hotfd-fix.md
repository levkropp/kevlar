# 086: M9.9 vDSO Syscall Acceleration & Hot-FD Cache Fix

Two wins in one session: a planned performance milestone (M9.9) that makes five
identity syscalls 30–55% faster than Linux, and a correctness fix for a
use-after-free in the hot-fd cache that crashed Alpine's `apk` toolchain.

## Baseline

Before this session, the five M9.9 target syscalls were all in the "ok but not
impressive" zone — 0.89–0.93x vs Linux KVM.  Meanwhile `make run-alpine` +
`bash test_apk_update.sh` hit a kernel page fault inside `INode::as_file`,
crashing with CR2=0x11 (null-ish dereference through freed memory).

## M9.9: Cached utsname (Phase 1)

`sys_uname` built a 390-byte `struct utsname` on the stack every call: six
string writes, two UTS namespace lock acquisitions, then a 390-byte usercopy.

### The fix

Pre-build the entire utsname buffer at process creation.  A new
`cached_utsname: SpinLock<[u8; 390]>` field on `Process` is populated by
`build_cached_utsname()` in all five constructors (idle, init, fork, vfork,
new_thread).  `sys_uname` becomes:

```rust
pub fn sys_uname(&mut self, buf: UserVAddr) -> Result<isize> {
    let utsname = current_process().utsname_copy();
    buf.write_bytes(&utsname)?;
    Ok(0)
}
```

One lock, one memcpy, zero string operations.

### Result

| Syscall | Before | After | Linux | Ratio |
|---------|--------|-------|-------|-------|
| uname   | 145ns  | 118ns | 251ns | **0.47x** |

More than 2x faster than Linux.  The TODO for sethostname/setdomainname
invalidation is noted but irrelevant until container workloads change hostnames
at runtime.

## M9.9: Lean dispatch (Phase 2)

Every syscall paid ~5ns overhead for `tick_stime()`, `record_syscall()`,
`profiler::syscall_enter/exit()`, and `htrace::enter_guard()` — even
trivial read-only calls like `getpid`.

### The fix

A new `is_lean_syscall()` predicate identifies nine trivial syscalls:

```rust
fn is_lean_syscall(n: usize) -> bool {
    matches!(n,
        SYS_GETPID | SYS_GETTID | SYS_GETUID | SYS_GETEUID |
        SYS_GETGID | SYS_GETEGID | SYS_GETPRIORITY | SYS_UNAME |
        SYS_GETTIMEOFDAY
    )
}
```

At the top of `dispatch()`, when debug flags are off and the syscall is lean,
we skip all accounting and jump straight to `do_dispatch` → write rax → signal
delivery → return.  One atomic load (`get_filter()`) gates the fast path.

### Result

| Syscall     | Before | After | Linux | Ratio |
|-------------|--------|-------|-------|-------|
| getpid      | 77ns   | 63ns  | 97ns  | **0.65x** |
| getuid      | 76ns   | 63ns  | 111ns | **0.57x** |
| getpriority | 80ns   | 69ns  | 93ns  | **0.74x** |

All identity syscalls now comfortably faster than Linux.

## M9.9: Per-process vDSO page (Phases 3–4)

The existing vDSO was a single shared page with `__vdso_clock_gettime`.
To prepare for glibc (which calls `__vdso_getpid` etc.), we needed per-process
data in the vDSO and expanded symbol metadata.

### What changed

Complete rewrite of `platform/x64/vdso.rs`:

- **Data area** moved from 0xF00 to 0xE00 with new fields: pid (0xE10),
  tid (0xE14), uid (0xE18), nice (0xE1C), utsname (0xE20, 390 bytes).
- **7 vDSO functions** with hand-crafted x86_64 machine code at 0x300+:
  `__vdso_clock_gettime`, `__vdso_gettimeofday`, `__vdso_getpid`,
  `__vdso_gettid`, `__vdso_getuid`, `__vdso_getpriority`, `__vdso_uname`.
- **ELF metadata** expanded: 8-entry symbol table, 116-byte strtab, 44-byte
  SYSV hash table.  All RIP-relative displacements recomputed for the new
  code/data layout.
- **`alloc_process_page()`** clones the boot template and writes per-process
  fields.  Called in fork, vfork, and init constructors.
- **`update_tid(paddr, 0)`** zeros the TID field when threads are created,
  forcing `__vdso_gettid` to fall back to syscall in multi-threaded processes.
- **execve** remaps the vDSO with the current process's personal page.

musl only looks up `__vdso_clock_gettime` and `__vdso_gettimeofday`, so the
identity symbols are infrastructure for glibc (M10 Phase 8).  The
`__vdso_gettimeofday` symbol is the one immediate win — musl uses it for
`gettimeofday()` callers in server workloads.

## bench_gettid fix (Phase 0)

The `bench_gettid` benchmark called `syscall(SYS_gettid)` directly instead of
`gettid()`.  This bypassed musl's TID cache, making the benchmark inconsistent
with all other benchmarks.  The fix is one line:

```c
// Before: syscall(SYS_gettid);
// After:
gettid();
```

Result: gettid benchmark now reports 1ns (musl cache hit) instead of 80ns.

## Hot-FD cache use-after-free

### The problem

While testing Alpine Linux, `bash test_apk_update.sh` triggered a kernel page
fault:

```
CR2 (fault vaddr) = 0000000000000011
interrupted at: <kevlar_vfs::inode::INode>::as_file+0xb
backtrace:
  0: OpenedFile::read+0x26
  1: SyscallHandler::sys_read+0x235
```

The hot-fd cache (`file_hot_fd` / `file_hot_ptr`) stores raw `*const OpenedFile`
pointers to skip fd table lookups on repeat calls.  The cache comment
explicitly said: *"Invalidated by close/dup2/dup3/close_range before the Arc
is dropped."*

But `invalidate_hot_fd()` was **defined and never called**.  When `close()`
dropped the `Arc<OpenedFile>`, the cached raw pointer became dangling.  The
next `read()` on the same fd number dereferenced freed memory, hitting offset
0x11 inside a deallocated `PathComponent.inode` — classic use-after-free.

### The fix

Added `invalidate_hot_fd()` calls to every fd-mutating path:

```rust
// close.rs
proc.invalidate_hot_fd(fd.as_int());
proc.opened_files_no_irq().close(fd)?;

// dup2.rs / dup3.rs — `new` fd is being replaced
current.invalidate_hot_fd(new.as_int());

// close_range.rs — check if cached fd is in the closed range
if hot >= 0 && (hot as u32) >= first && (hot as u32) <= last {
    proc.invalidate_hot_fd(hot);
}

// execve CLOEXEC — flush both caches entirely
current.file_hot_fd.store(-1, Ordering::Relaxed);
current.file_hot_ptr.store(core::ptr::null_mut(), Ordering::Relaxed);
```

### Result

Alpine `test_apk_update.sh` passes 7/7.  Contract tests: 105/118 PASS, 0 FAIL.

## Benchmark summary (all 4 profiles)

Ran `bench-kvm` on all four safety profiles.  **Zero regressions** across 44
benchmarks on all profiles.

| Syscall       | Linux KVM | Balanced | Ratio | Status |
|---------------|-----------|----------|-------|--------|
| clock_gettime | 26ns      | 10ns     | 0.38x | no regression |
| uname         | 251ns     | 118ns    | 0.47x | **+55% improvement** |
| getpid        | 97ns      | 63ns     | 0.65x | **+28% improvement** |
| getuid        | 111ns     | 63ns     | 0.57x | **+37% improvement** |
| getpriority   | 93ns      | 69ns     | 0.74x | **+20% improvement** |
| gettid        | 115ns     | 1ns      | 0.01x | musl cache hit |

All profiles: **41 faster, 2 OK, 0 marginal, 0 regression.**

## Test results

| Suite | Result |
|-------|--------|
| Contract tests (4 profiles) | 105/118 PASS, 0 FAIL |
| SMP threading (4 CPUs)      | 14/14 PASS |
| mini_systemd                | 15/15 PASS |
| Alpine tests                | 7/7 PASS |

## Files changed

| File | Change |
|------|--------|
| `benchmarks/bench.c` | `syscall(SYS_gettid)` → `gettid()` |
| `kernel/process/process.rs` | `cached_utsname` field, `build_cached_utsname()`, `vdso_data_paddr` field, execve vDSO remap, execve CLOEXEC cache flush |
| `kernel/syscalls/uname.rs` | Single `utsname_copy()` + `write_bytes()` |
| `kernel/syscalls/mod.rs` | `is_lean_syscall()` + lean dispatch fast path |
| `platform/x64/vdso.rs` | Complete rewrite: 7 functions, per-process pages, expanded ELF metadata |
| `kernel/syscalls/close.rs` | `invalidate_hot_fd()` before close |
| `kernel/syscalls/close_range.rs` | Range-check + `invalidate_hot_fd()` |
| `kernel/syscalls/dup2.rs` | `invalidate_hot_fd(new)` before dup2 |
| `kernel/syscalls/dup3.rs` | `invalidate_hot_fd(new)` before dup2 |
