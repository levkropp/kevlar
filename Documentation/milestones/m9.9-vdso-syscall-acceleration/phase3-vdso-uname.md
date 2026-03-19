# M9.9 Phase 3: vDSO uname

**Target:** uname ~40ns (vs Linux 162ns = 0.25x)

## Overview

`uname(2)` returns a 390-byte `struct utsname` containing 6 fixed-length
strings (sysname, nodename, release, version, machine, domainname). These
are static for the lifetime of a process (nodename/domainname can change
via UTS namespace, but not mid-syscall).

The current implementation (`kernel/syscalls/uname.rs`) builds the struct
on the stack and does a single 390-byte usercopy. The syscall overhead
(entry/exit/dispatch) dominates — the actual work is trivial.

A vDSO implementation pre-builds the 390-byte struct in the vDSO data page
and does a single `rep movsb` copy to the user buffer, avoiding the syscall
entirely.

## Data page layout

```
0xF20: utsname (390 bytes) — pre-built struct utsname
  0xF20 + 0*65: sysname    "Kevlar"
  0xF20 + 1*65: nodename   "(none)" or hostname
  0xF20 + 2*65: release    "6.1.0-kevlar"
  0xF20 + 3*65: version    "#1 SMP ..."
  0xF20 + 4*65: machine    "x86_64"
  0xF20 + 5*65: domainname "(none)"
```

Total data: 0xF20 + 390 = 0x10A6. This fits within the 4KB page (ends at
0x1000). Actually 0xF20 + 390 = 0x10A6 overflows! Need to move data or
use a second page.

**Solution**: The code area (0x200-0xEFF) is mostly empty — the current
clock_gettime code is only 73 bytes. Move the data area start from 0xF00
to 0xE00, giving 512 bytes of data space. Layout:

```
0xE00: tsc_origin  (u64)
0xE08: ns_mult     (u64)
0xE10: pid         (i32)
0xE14: tid         (i32)
0xE18: uid         (u32)
0xE1C: nice        (i32)
0xE20: utsname     (390 bytes) → ends at 0xFA6
```

0xFA6 < 0x1000 — fits. Code at 0x200-0x400 has plenty of room for all
5 new functions (~80 bytes total).

**Alternative**: Keep 0xF00 data area but reduce UTS_FIELD_LEN to 33
(enough for all our strings). 6*33 = 198 bytes → fits in 0xF20-0xFE6.
Downside: ABI incompatible — userspace expects 65-byte fields.

**Decision**: Move data area to 0xE00. Update all RIP-relative offsets
in existing clock_gettime code.

## vDSO code

```asm
; int __vdso_uname(struct utsname *buf)
; Copy 390 bytes from vDSO data to user buffer.
__vdso_uname:
    lea rsi, [rip + OFF_UTSNAME]  ; source: vDSO data
    mov rdi, rdi                   ; dest: user buffer (already in rdi)
    ; Swap rsi/rdi: rep movsb copies from [rsi] to [rdi]
    ; rdi = dest (user buf), rsi = source (vDSO data)
    ; Wait — arguments are: rdi = buf (user buffer). We need:
    ;   rdi = destination = buf
    ;   rsi = source = vDSO data
    lea rsi, [rip + OFF_UTSNAME]
    mov ecx, 390
    rep movsb
    xor eax, eax                   ; return 0
    ret
```

~15 bytes. The `rep movsb` is optimized on modern CPUs (ERMS) and copies
390 bytes in ~20ns.

## Kernel-side writes

### On fork / new_init_process
```rust
// Build utsname in the vDSO data page
let utsname_ptr = vdso_data.add(OFF_UTSNAME);
write_uts_field(utsname_ptr, 0, b"Kevlar");
write_uts_field(utsname_ptr, 1, hostname.as_bytes());
write_uts_field(utsname_ptr, 2, b"6.1.0-kevlar");
write_uts_field(utsname_ptr, 3, b"#1 SMP");
write_uts_field(utsname_ptr, 4, b"x86_64");
write_uts_field(utsname_ptr, 5, b"(none)");
```

### On UTS namespace hostname change
If Kevlar implements `sethostname(2)`, update the vDSO page for all
processes in that UTS namespace. Currently Kevlar has UTS namespaces
but hostname is static, so this is a future concern.

## ELF metadata

Add 1 more symbol: `__vdso_uname\0`. Update hash table.

## Verification

```bash
make check
make bench-kvm             # uname < 50ns
make test-contracts        # 105 PASS, 0 FAIL
```

## Risk: musl uname behavior

musl's `uname()` calls the raw syscall directly — it does NOT check for a
vDSO symbol. So the vDSO version will only be used if musl is patched or
if we intercept at the syscall level.

**Mitigation**: Instead of a vDSO function, we could make the kernel-side
`sys_uname` faster by pre-building the struct at fork time and doing a
single page-aligned copy. This avoids the musl dependency entirely.

**Alternative**: The benchmark calls `uname()` via musl which issues
`SYS_uname`. We can make the kernel-side handler faster by pre-building
the utsname buffer on the Process struct (once at fork/exec, not on every
call). This eliminates the per-call string formatting and gets us to ~60ns
(single usercopy of 390 bytes).

**Recommendation**: Do both — pre-build in kernel (guaranteed win) and
add vDSO symbol (wins for any future libc that checks for it).
