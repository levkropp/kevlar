# Blog 117: OpenRC INVALID_OPCODE — signal delivery fix and crash investigation

**Date:** 2026-03-24
**Milestone:** M10 Alpine Linux

## Summary

Fixed the kernel's user fault signal delivery (all exceptions sent SIGSEGV;
now correctly sends SIGILL, SIGFPE, etc.) and investigated a deterministic
INVALID_OPCODE crash in OpenRC's "Caching service dependencies" phase.
The crash is caused by the CPU executing from the middle of a valid `mov`
instruction in musl's timezone code — a 2-byte RIP misalignment that points
to a signal return or page fault return bug.

Also fixed: UDP `getsockname` (c-ares DNS), certificate verification tests
targeting google.com (Alpine CA bundle coverage), and the `test-openssl`
Makefile target timeout.

## Bug Fix: User fault signal types

All user-mode CPU exceptions were unconditionally mapped to SIGSEGV and
killed the process immediately via `exit_by_signal()`. This meant:
- Programs couldn't install SIGILL handlers (e.g., for CPU feature probing)
- SIGFPE handlers for divide-by-zero never fired
- The signal number in `waitpid` status was wrong (11 instead of 4/8)

**Fix** (`kernel/main.rs`): Map exception vectors to POSIX signals:
| Exception | Signal |
|-----------|--------|
| INVALID_OPCODE | SIGILL (4) |
| DIVIDE_ERROR | SIGFPE (8) |
| X87_FPU, SIMD_FLOATING_POINT | SIGFPE (8) |
| GPF, stack/segment faults | SIGSEGV (11) |

Changed `exit_by_signal(SIGSEGV)` to `send_signal(correct_signal)` — the
signal is now delivered through the normal path, allowing user handlers to
catch faults. If no handler is installed (SIG_DFL = terminate), the process
dies on interrupt return via `x64_check_signal_on_irq_return`.

## OpenRC Crash Investigation

### The symptom

OpenRC boots, creates `/run/openrc` and `/run/lock`, starts "Caching service
dependencies", then crashes:
```
USER FAULT: INVALID_OPCODE pid=7 ip=0xa000411f1 signal=4 cmd=/sbin/openrc sysinit
```

### Identifying the crash location

1. **Interpreter base**: Added PID-tagged logging to `execve()` →
   OpenRC's ld-musl loads at `0xa0000b000`

2. **Offset**: `0xa000411f1 - 0xa0000b000 = 0x361f1` in ld-musl

3. **Function**: `sem_close+0xf71` — actually musl's timezone/localtime
   implementation (objdump mis-labels due to stripped symbols)

4. **Instruction**: The crash is 2 bytes INTO a valid 6-byte instruction:
   ```
   361ef: 8b 05 37 ee 06 00    mov    0x6ee37(%rip),%eax
   361f5: f7 d8                neg    %eax
   ```
   At IP `0x361f1`, the CPU sees byte `0x37` — the removed AAA instruction,
   invalid in 64-bit mode → #UD (invalid opcode)

### Verifying memory content

Read the actual bytes from process memory via the kernel fault handler:
```
code at ip: 37 ee 06 00 f7 d8 48 98 49 89 04 24 48 8b 05 8c
```
**Matches the file exactly.** Demand paging loaded the correct bytes.
The CPU really IS executing from the middle of a valid instruction.

### Register state at crash

```
RIP=0x0000000a000411f1  RSP=0x00000009ffffd3f8  RBP=0x0000000000000001
RAX=0x0000000000000000  RBX=0x0000000a001a9030  RCX=0x0000000a000411f1
RDX=0x0000000000000000  RSI=0x0000000000000000  RDI=0x0000000000000011
R12=0x00000009ffffd80f  R13=0x0000000a000cd0b0  R14=0x0000000000000000
```

Key observation: **RCX == RIP**. On x86-64, `syscall` sets RCX = return
address. This suggests the crash address was the return point from a prior
syscall, and the register was never overwritten.

### Stack analysis

```
[+0]  = 0x0000000a001255a4   (data — not a return address)
[+8]  = 0x0000000000000000
[+16] = 0x0000000a0006a3be   (return from __overflow → after syscall at 0x5f3bc)
[+24] = 0x00000009ffffd7c0   (saved RBP)
```

The `__overflow` function at `0x5f3bc` has a `syscall` instruction —
this is musl's `write()` syscall wrapper called during stdio flushing.

### What the `mov` instruction accesses

The faulting `mov 0x6ee37(%rip),%eax` reads from virtual address `0xa502c`
(RIP-relative), which is in musl's **BSS** (zero-initialized data, not in
the file). If this page isn't mapped yet, a demand page fault occurs.

### Leading hypothesis: signal return corrupts RIP

The crash site is in timezone code called during `localtime()`. OpenRC
forks child processes to scan `/etc/init.d/`, and these children exit,
generating SIGCHLD signals. If SIGCHLD arrives while the parent is
executing the `mov` instruction at `0x361ef`:

1. CPU is at RIP=`0x361ef`, executing `mov 0x6ee37(%rip),%eax`
2. SIGCHLD is pending — signal delivery saves RIP to the signal frame
3. Signal handler runs, calls `rt_sigreturn`
4. **Bug**: `sigreturn` restores RIP as `0x361f1` instead of `0x361ef`
   (2-byte offset error)
5. CPU resumes at `0x361f1` → byte `0x37` → INVALID_OPCODE

The 2-byte offset matches the size of `syscall` (0f 05) — the signal
delivery code might be confusing the faulting instruction address with a
post-syscall return address.

### Diagnostic tooling built

- **`crash_handler.c`**: LD_PRELOAD library with `__attribute__((constructor))`
  that installs SIGILL/SIGSEGV/SIGBUS handlers printing registers and code
  bytes. Didn't fire because OpenRC forks+exec's helpers which reset handlers.
- **Kernel register dump**: Added register and code-byte dump to the
  `handle_user_fault` path.
- **PID-tagged interpreter logging**: `interp: pid=7 base=0xa0000b000`

## Status

| Suite | Result |
|-------|--------|
| Contract tests | **159/159 PASS** |
| M10 APK (ext2) | **7/7 PASS** |
| ext4 comprehensive | **29/29 PASS** |
| OpenSSL/TLS | **18/18 PASS** |

## Root Cause Found via GDB (update)

### Autonomous GDB tooling

Built `tools/gdb-investigate.py` — a general-purpose autonomous GDB crash
debugger for Kevlar:
- Patches kernel ELF with init path
- Starts QEMU with KVM + GDB stub
- Connects GDB, sets hardware breakpoints, runs Python scripts
- Outputs structured JSON for analysis
- `make gdb-investigate BREAK=0x... STEP=20` Makefile target

### GDB trace sequence

1. **Break at `sysretq`**: Found that RCX = `0xa000411f1` (the crash address)
   right before `sysretq` executes — confirming the kernel returns to the
   wrong user-mode address.

2. **Break at `syscall_entry`**: The SAME process entered `wait4` with
   RCX = `0xa0012f347` (the CORRECT return address). So `frame.rip` changed
   DURING the `wait4` sleep.

3. **PtRegs dump at crash**: `frame.rcx = 0xa0012f347` (correct, set by
   hardware `syscall`) but `frame.rip = 0xa000411f1` (corrupted). These
   are pushed from the SAME register at syscall entry — they should be
   identical.

4. **Stack search**: `0xa000411f1` appears 3 more times in the kernel stack
   below the PtRegs frame. This value is a legitimate ld-musl timezone code
   address that gets written as a local variable during the wait4/scheduler
   code path, and accidentally overwrites `frame.rip`.

### Definitive root cause

**Kernel stack corruption during `wait4` sleep.** The syscall frame's
`rip` field (offset +128 in PtRegs) is overwritten by a legitimate code
address (`0xa000411f1` = musl timezone code) that lives on the same kernel
stack as a local variable. The scheduler or wait queue code's deep call
chain + timer interrupt frames overlap with the PtRegs area.

### Next step

Find the exact write that corrupts `frame.rip` — either:
- Set a hardware write watchpoint on the `frame.rip` stack address
- Increase kernel stack size from 2-page to 4-page usable region
- Audit the `sleep_signalable_until` → scheduler → context switch call
  depth for stack overflow potential
