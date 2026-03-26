# Blog 118: OpenRC crash root cause — bogus signal handler from dynamic linker relocation bug

**Date:** 2026-03-24
**Milestone:** M10 Alpine Linux

## Summary

The OpenRC INVALID_OPCODE crash that has persisted since Alpine integration
was traced to its root cause using autonomous GDB tooling: a **dynamic linker
relocation bug** causes OpenRC's SIGCHLD handler to point to a mid-instruction
address in musl's timezone code. The handler address `0xa000411f1` is an
unrelocated function pointer from librc.so.1 — musl's dynamic linker failed
to apply the base address relocation when loading the library.

## GDB Investigation Sequence

### Phase 1: sysretq trace

Hardware breakpoint at `sysretq` (`0xffff8000001013f5`) with conditional
check: only stop when `RCX == 0xa000411f1`.

**Result**: At iteration 29, `sysretq` about to execute with
`RCX = 0xa000411f1` — the kernel IS returning to the wrong address.

### Phase 2: Syscall entry vs exit

Hardware breakpoints at both `syscall_entry` and `pop rcx` (before sysretq).
Track `wait4` calls (syscall 61) from PIE processes.

**Result**: Same process entered `wait4` with `RCX = 0xa0012f347` (correct
return address), but `frame.rip = 0xa000411f1` at exit. **The frame.rip was
corrupted during wait4 execution.**

### Phase 3: PtRegs frame dump

Read the full PtRegs at the `pop rcx` breakpoint:

```
frame.rcx = 0xa0012f347  ← correct (set by syscall hardware)
frame.rip = 0xa000411f1  ← CORRUPTED (should equal rcx)
orig_rax  = 0x3d (61)    ← wait4 syscall number
```

`frame.rcx` and `frame.rip` are pushed from the SAME register at syscall
entry (`push rcx` in usermode.S) — they should be identical. The fact
that they differ proves something wrote to `frame.rip` after entry.

### Phase 4: Hardware write watchpoint

Set a write watchpoint on the exact memory address of `frame.rip` in the
kernel stack (`0xffff80003ff47fd8`).

**Result**: The watchpoint fired at:

```
#0  setup_signal_stack (frame=..., signal=17, ...)
#1  try_delivering_signal (frame=...)
#2  SyscallHandler::dispatch (...)
#3  handle_syscall (..., n=61, frame=...)
```

**Signal 17 = SIGCHLD** was being delivered during the `wait4` syscall's
return path. `setup_signal_stack` wrote the SIGCHLD handler address
(`0xa000411f1`) into `frame.rip`, which `sysretq` then jumped to.

## The bogus handler address

The handler `0xa000411f1` is at offset `0x361f1` in ld-musl — the middle
of a `mov 0x6ee37(%rip),%eax` instruction in timezone code. Byte `0x37`
(the old AAA instruction) is invalid in 64-bit mode → #UD.

Kernel-level tracing of `rt_sigaction` confirmed userspace IS passing this
exact address:

```
rt_sigaction: SIGCHLD handler=0xa000411f1 flags=0x4000000 restorer=0xa000411a4 pid=2
```

Both handler and restorer are in the same ~80-byte range of musl's timezone
code — neither is a valid function entry point.

## musl's sigaction wrapper

Disassembly of musl's `sigaction` function at offset `0x5dfd9` shows:

```asm
5df3b: lea    0x662(%rip),%rax        # 5e5a4 ← __restore_rt
5df42: mov    %rax,0x10(%rsp)         # ksa.restorer = __restore_rt
```

The `lea` correctly computes `__restore_rt = 0x5e5a4` via RIP-relative
addressing. With interp base `0xa0000b000`, the correct restorer would be
`0xa000695a4`. But userspace passes `0xa000411a4` (offset `0x361a4`).

**The difference: `0x5e5a4 - 0x361a4 = 0x28400` (164 KB)**

This means the handler and restorer addresses are **unrelocated or
mis-relocated function pointers** — the base address wasn't properly
added to the raw offset.

## Root cause: dynamic linker relocation

The SIGCHLD handler comes from **librc.so.1** (OpenRC's service management
library). When musl's dynamic linker loads librc.so.1 via `mmap`, it must
apply RELR/RELA relocations to fix up function pointers in the library's
data segment.

If a function pointer in librc's data (e.g., a signal handler callback
stored in a struct) isn't relocated, it retains its pre-relocation value
(a small offset). When OpenRC passes this unrelocated pointer to
`sigaction()`, the kernel stores a bogus address.

### Why other programs work

Most programs (BusyBox, curl, test binaries) either:
- Don't install SIGCHLD handlers
- Use statically-linked signal handlers (no relocation needed)
- Use libraries that don't store signal handler pointers in relocated data

OpenRC is unusual: it uses librc.so.1 which has signal handler function
pointers in its data segment that require RELR relocation.

## GDB tooling built

### `tools/gdb-investigate.py`

General-purpose autonomous GDB crash debugger:
- Hardware breakpoints on kernel symbols (works under KVM)
- Python script generation for automated breakpoint handling
- Conditional breakpoints (check register values before stopping)
- PtRegs frame dumping, stack search, JSON output
- Makefile target: `make gdb-investigate BREAK=0x... STEP=20`

### Investigative techniques used

| Technique | What it found |
|-----------|---------------|
| hbreak at sysretq | RCX contains the crash address |
| hbreak at syscall_entry + pop rcx | frame.rip changes during wait4 sleep |
| PtRegs dump at pop rcx | rcx ≠ rip in same frame (corruption proof) |
| write watchpoint on frame.rip | setup_signal_stack writing SIGCHLD handler |
| rt_sigaction kernel trace | userspace passes bogus handler address |
| musl disassembly | lea correctly computes __restore_rt |

## Other changes in this session

### Signal type mapping (kept)

`handle_user_fault` now sends the correct POSIX signal for each x86
exception type: INVALID_OPCODE → SIGILL, DIVIDE_ERROR → SIGFPE, etc.
Previously all exceptions sent SIGSEGV.

### kernel_stack for syscalls (reverted)

Attempted to use the 16KB `kernel_stack` for `head.rsp0` instead of the
8KB `syscall_stack`. This was based on an initial (incorrect) hypothesis
that the crash was a stack overflow. The change caused signal delivery
regressions because `head.rsp0` isn't initialized before the first
`switch_task` call. Reverted — the real fix is the dynamic linker.

## Status

| Suite | Result |
|-------|--------|
| Contract tests | **159/159 PASS** |
| M10 APK (ext2) | **7/7 PASS** |
| ext4 comprehensive | **29/29 PASS** |
| OpenSSL/TLS | **18/18 PASS** |

## Next step

Investigate Kevlar's demand paging RELR relocation for mmap'd shared
libraries. The dynamic linker (ld-musl) loads librc.so.1 via `mmap` and
then applies relocations. If Kevlar's `mmap` or page fault handler
interferes with the relocation process (e.g., by prefaulting pages with
stale data before relocations are applied), function pointers in the
library's data segment would be wrong.
