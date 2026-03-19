# M9.9 Phase 1: vDSO getpid + gettid

**Target:** getpid ~10ns, gettid ~10ns (vs Linux 86ns, 90ns)

## Overview

Add `__vdso_getpid` and `__vdso_gettid` to the vDSO page. Both read
pre-written values from the vDSO data area, avoiding the kernel entirely.

musl calls the raw `SYS_getpid` / `SYS_gettid` syscall every time (no libc
caching), so the vDSO intercept gives us the full speedup.

## Data page layout extension

Current layout (offset 0xF00):
```
0xF00: tsc_origin  (u64)  — clock_gettime
0xF08: ns_mult     (u64)  — clock_gettime
```

Extended layout:
```
0xF00: tsc_origin  (u64)  — clock_gettime
0xF08: ns_mult     (u64)  — clock_gettime
0xF10: pid         (i32)  — getpid (tgid, namespace-local)
0xF14: tid         (i32)  — gettid (thread-local PID)
0xF18: uid         (u32)  — getuid  (Phase 2)
0xF1C: nice        (i32)  — getpriority (Phase 2)
0xF20: uname_data  (390B) — uname (Phase 3)
```

## vDSO code: __vdso_getpid

```asm
; pid_t __vdso_getpid(void)
; Returns the PID (tgid) from the vDSO data page.
__vdso_getpid:
    movsxd rax, dword [rip + OFF_PID]   ; sign-extend i32 → i64
    ret
```

~6 bytes of code. The `movsxd` handles sign extension for the syscall
return value convention (returns `long`).

## vDSO code: __vdso_gettid

```asm
; pid_t __vdso_gettid(void)
; Returns the TID from the vDSO data page.
__vdso_gettid:
    movsxd rax, dword [rip + OFF_TID]
    ret
```

~6 bytes. Same pattern.

## Per-process vDSO data page

Currently all processes share the same physical vDSO page (one allocation
at boot). To serve per-process PID/TID, each process needs its own copy:

1. **`vdso::alloc_process_page()`**: Allocate a fresh 4KB page, copy the
   boot template (code + ELF metadata), then write process-specific data.
2. **Call sites**: `Process::new_init_process()`, `Process::fork()`,
   `Process::vfork()`, `Process::new_thread()`.
3. **Mapping**: `map_vdso_page()` in VM setup uses the per-process paddr
   instead of the global one.

For threads (`new_thread` with CLONE_VM), TID differs per-thread but the
vDSO page is shared (same address space). Two approaches:
- **Approach A**: Each thread gets its own vDSO page mapping. Requires
  per-thread PML4 entry (heavyweight, 4KB/thread).
- **Approach B**: Store TID at a per-thread location the vDSO code can
  read. Use the TLS area (FS segment base + fixed offset), or a dedicated
  per-CPU page.
- **Approach C**: `__vdso_gettid` stores TID in the shared page but falls
  back to `syscall(SYS_gettid)` when multiple threads exist. Since the
  benchmark is single-threaded, this gives full speedup for the common case.

**Recommended: Approach C** (simplest, handles benchmark case, correct
fallback). Implementation:
- Write TID to vDSO data page on `fork()`/`exec()`
- `__vdso_gettid` reads it unconditionally (correct for single-threaded)
- When `clone(CLONE_THREAD)` is called, set a flag in the data page
  that makes `__vdso_gettid` fall back to syscall
- gettid benchmark runs single-threaded → full vDSO speed

## Kernel-side writes

### On fork / new_init_process
```rust
// After allocating per-process vDSO page:
let vdso_data = vdso_page_paddr.as_vaddr().as_mut_ptr::<u8>();
unsafe {
    // Write PID (tgid) at offset 0xF10
    (vdso_data.add(0xF10) as *mut i32).write(ns_pid);
    // Write TID at offset 0xF14
    (vdso_data.add(0xF14) as *mut i32).write(ns_pid); // tid == pid for leader
}
```

### On clone(CLONE_THREAD)
```rust
// Mark vDSO data page as multi-threaded → gettid falls back to syscall
unsafe {
    // Set a "thread_count > 1" flag or simply set TID to 0 (invalid)
    // to force syscall fallback
    (vdso_data.add(0xF14) as *mut i32).write(0);
}
```

## ELF metadata changes

The vDSO ELF must export two new symbols. Update:
- **String table**: Add `__vdso_getpid\0` and `__vdso_gettid\0`
- **Symbol table**: Add 2 new Elf64_Sym entries (STB_GLOBAL | STT_FUNC)
- **Hash table**: Expand to 3 symbols (nchain=4, including null symbol)

musl's `__vdsosym()` iterates all symbols checking for name match, so the
hash table just needs correct nchain count.

## Verification

```bash
make check                 # type-check
make bench-kvm             # getpid < 15ns, gettid < 15ns
make test-contracts        # 105 PASS, 0 FAIL
make test-m6               # threading tests still pass
```
