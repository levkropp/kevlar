# M9.9 Phase 2: vDSO getuid + getpriority

**Target:** getuid ~10ns, getpriority ~10ns (vs Linux 84ns, 86ns)

## Overview

Add `__vdso_getuid` and `__vdso_getpriority` to the vDSO data page.
Same pattern as Phase 1 — read pre-written values from fixed offsets.

## Data page fields

```
0xF18: uid   (u32)  — effective UID (getuid returns real UID, but Kevlar
                       sets uid == euid on setuid)
0xF1C: nice  (i32)  — nice value for getpriority(PRIO_PROCESS, 0)
```

## vDSO code

```asm
; uid_t __vdso_getuid(void)
__vdso_getuid:
    mov eax, dword [rip + OFF_UID]   ; zero-extends u32 → u64
    ret

; int __vdso_getpriority(int which, id_t who)
; Only handles PRIO_PROCESS(0), who=0 (current process).
; Falls back to syscall for other arguments.
__vdso_getpriority:
    test edi, edi             ; which == PRIO_PROCESS (0)?
    jnz .fallback
    test esi, esi             ; who == 0 (self)?
    jnz .fallback
    movsxd rax, dword [rip + OFF_NICE]
    ; Linux getpriority returns 20-nice (so 20 for nice=0, 1 for nice=19)
    neg eax
    add eax, 20
    ret
.fallback:
    mov eax, 140              ; SYS_getpriority
    syscall
    ret
```

## Kernel-side updates

### Write on fork
```rust
(vdso_data.add(0xF18) as *mut u32).write(uid);
(vdso_data.add(0xF1C) as *mut i32).write(nice);
```

### Update on setuid / setpriority
```rust
// In sys_setuid / sys_setpriority:
// After updating the process field, also update the vDSO data page.
if let Some(vdso_paddr) = proc.vdso_data_paddr() {
    let ptr = vdso_paddr.as_vaddr().as_mut_ptr::<u8>();
    unsafe { (ptr.add(0xF18) as *mut u32).write(new_uid); }
}
```

## ELF metadata

Add 2 more symbols to the symbol table and string table:
- `__vdso_getuid\0`
- `__vdso_getpriority\0`

Update hash table nchain accordingly.

## Verification

```bash
make check
make bench-kvm             # getuid < 15ns, getpriority < 15ns
make test-contracts        # 105 PASS, 0 FAIL
```

## Note on getpriority semantics

Linux `getpriority(2)` returns `20 - nice` (range 1-40, never negative/zero)
to disambiguate from error returns. The vDSO must replicate this transform.
The kernel-side `sys_getpriority` already does this — verify the benchmark
calls the right thing.
