## Blog 180: the CLD bug — one missing instruction, two months of stack corruption

**Date:** 2026-04-19

For the better part of two months we'd been chasing an intermittent
~30% kernel-page-fault crash during XFCE startup. The signature was
ugly:

```
kernel page fault — register dump:
  RIP=0000000000000002  RSP=ffff800038509b78  RBP=deadcafedeadcafe
  RAX=ffff800038509b78  RBX=0000000000000002  RDX=0000000000000548
  RSI=ffff800038509b80  RDI=ffff800038509630  R8 =0000000000000008
  R10=00007ffffd379eef  R11=0000000000000013
  CS=0x8 (ring 0)  CR2 (fault vaddr) = 0000000000000002
```

`RIP=2`, kernel-mode, instruction-fetch fault. `RBP=0xdeadcafedeadcafe`
which is the exact `GUARD_MAGIC` poison written into the bottom 512
bytes of every kernel stack. `RAX=RSP` exactly. `RBX=2` matches `RIP`.
`R10` and `R9` look like userspace pointers, `R11=0x13` looks like
a syscall-shape RFLAGS.

We tried six hypotheses across three sessions:

1. **Saved-context corruption on suspended tasks** — wrote a periodic
   scanner of every `BlockedSignalable`/`Stopped` task's
   `do_switch_thread`-saved frame. After persistence-filtering, log-only
   mode showed 0 real hits. Detector-induced races, dead end.
2. **Buddy double-allocation of a live stack** — wrote
   `STACK_REGISTRY` tracking every live kernel-stack paddr and a
   `check_not_stack` panic in `alloc_pages`. 0 hits across 8 runs.
3. **Bulk `zero_page()` corruption** — disproved by the size: the
   corruption is one specific 8-byte slot, not 4 KB.
4. **Indirect call/jmp through a corrupted function pointer** —
   disassembled `refill_page_cache` and the surrounding allocator
   paths. No indirect dispatch, all CALLs are direct.
5. **Consecutive PAddrs in the dump being the bug** — the consecutive
   physical addresses were stale leftover data ABOVE the popped slot
   (refill_page_cache's freed `buf[64]`), not the active corruption.
6. **Single-byte stack-slot zeroing of valid kernel pointers** — wrote
   `scan_live_stack_corruption` looking for the specific
   `0x00ff_8000_xxxx_xxxx` pattern (kernel pointer with top byte
   cleared). Hits everywhere. **All benign — DIRTY_OK kernel-stack
   page recycling residue.** Crash rate unchanged.

After hypothesis 6 was disproved, the natural next move was to identify
*the function* that crashed, not what wrote the bad value. Reading the
register state more carefully:

- `RDI = RSP - 0x548`, `RSI = RSP + 8`, `RDX = 0x548`, `R8 = 8` —
  these are **memcpy/memmove arguments**: dst, src, len.
- `RAX = RSP` — function returned with `mov rax, rsp` or similar.
- `R13 = RSP + 0x548`, `R14 = RSP`, `R15 = RSP + 0xa98` — registers
  set up symmetrically around RSP, suggesting a function with two
  ~0x548-byte stack regions.

That's a function doing a 1352-byte stack-to-stack memcpy. And the
kernel's `memcpy` / `memset` on x86_64 use `rep movsb` / `rep stosb`:

```rust
// platform/mem.rs
pub unsafe extern "C" fn memset(dest: *mut u8, c: i32, n: usize) -> *mut u8 {
    core::arch::asm!(
        "rep stosb",
        inout("rdi") dest => _,
        inout("rcx") n => _,
        in("al") c as u8,
        options(nostack),
    );
    dest
}
```

`rep stosb` writes `n` bytes of `al` starting at `[rdi]`. Direction
depends on `RFLAGS.DF`:

- `DF=0` (CLD): `rdi += 1` each step — forward copy.
- `DF=1` (STD): `rdi -= 1` each step — backward copy.

The x86_64 SysV ABI specifies that DF must be 0 across function calls.
The kernel never sets `STD`. So our `memset` *should* always go
forward. Right?

## The leak

User code is allowed to set DF. glibc's `memmove` for backward
overlapping copy uses exactly this pattern:

```
std
rep movsb
cld
```

If a signal interrupts the user process between the `std` and the
`cld`, the signal handler runs with `DF=1`. If that handler invokes
a syscall (and signal handlers commonly call into libc which calls
into syscalls), the kernel inherits `DF=1`.

What does the kernel do with DF on entry? Let's check
`platform/x64/usermode.S`:

```
syscall_entry:
    swapgs
    mov gs:[GS_RSP3], rsp
    mov rsp, gs:[GS_RSP0]
    push 32           // User SS
    push gs:[GS_RSP3] // User RSP
    ...
```

No `cld`. And `platform/x64/trap.S`:

```
interrupt_common:
    test qword ptr [rsp + 24], 3
    jz 1f
    swapgs
1:
    xchg rdi, [rsp]
    push r15
    ...
```

Also no `cld`.

So if the user enters the kernel with `DF=1`, the entire syscall
handler runs with `DF=1`. The next time anything inside the kernel
calls `memset`, `rep stosb` runs **backward**, zeroing memory below
`rdi` instead of above it. That backward write can clobber:

- The function's own saved RBP, callee-saved registers, return address.
- Outer functions' frames.
- A nearby kernel stack's guard region (the bottom 512 bytes
  preinitialized to `0xDEAD_CAFE_DEAD_CAFE`), which is why we kept
  seeing `RBP=0xdeadcafe` in the crash dumps — the backward sweep
  *partially* overwrote a guard region, and the residue we read was
  the original `GUARD_MAGIC` that wasn't yet overwritten.

The same applies to `rep movsb` in `memcpy`.

## The fix

Two `cld` instructions. One in each kernel entry path:

```diff
 syscall_entry:
     swapgs
+
+    // Clear DF: x86_64 SysV ABI requires DF=0 across function calls,
+    // but user code is allowed to set it (e.g., glibc memmove uses
+    // std + rep movsb + cld; a signal interrupting between std and
+    // cld would enter the kernel with DF=1). Without cld here, kernel
+    // `rep stosb`/`rep movsb` (used by memset/memcpy) would run
+    // backward, corrupting memory BELOW the intended target —
+    // including saved RIP slots on the kernel stack.
+    cld
+
     mov gs:[GS_RSP3], rsp
```

```diff
     test qword ptr [rsp + 24], 3
     jz 1f
     swapgs
 1:
+    // Clear DF unconditionally: user could have left it set, and a
+    // kernel-mode interrupt mid-`rep movsb` would inherit the kernel's
+    // own DF=0, but we never want to assume.
+    cld
+
     xchg rdi, [rsp]
```

Linux's `arch/x86/entry/entry_64.S` does exactly this. Boot.S already
had a CLD at startup; we'd just trusted it to stay set forever, never
considering that the user could leave DF=1 across a syscall.

## The result

| Before fix | After fix |
|---|---|
| 3/8 crashes (~37%) | **0/10 crashes** |
| Threading: 14/14 | Threading: 14/14 |

XFCE startup completes cleanly every run. The `RIP=0/2 RBP=0xdeadcafe`
class of crash is gone.

## Lessons

The bug had been there since SMP came online (probably earlier). It
manifested only when:

1. A user process ran code that briefly set DF=1 (glibc memmove on
   overlapping ranges).
2. A signal arrived during that brief window.
3. The signal handler (or libc on its behalf) made a syscall.
4. The kernel happened to call `memset` or `memcpy` during that syscall.

XFCE startup hits all four conditions because it forks ~30 processes,
each doing extensive memory operations and signal handling, in parallel
on -smp 2. That's why the bug surfaced here and not in our threading
benchmarks: the threading suite doesn't trigger glibc's backward
memmove and doesn't generate enough signal traffic.

The six hypotheses we ruled out weren't *wrong* — they were checking
the *wrong layer*. The corruption signature looked like "single byte
of a kernel pointer cleared" because the backward `rep stosb` started
from a stack address with high byte 0xff and only swept a few bytes
before hitting the natural end of its `rcx` count. The "consecutive
PAddrs" were genuinely from `refill_page_cache::buf[64]` — but that
function wasn't the source of the corruption, it was the *victim*
(its memset on its local buffer ran backward, clobbering its own
saved return address).

The diagnostic infrastructure built during the chase isn't wasted.
The live-stack scanner, the suspended-task corruption detector, the
extended `switch_task` saved-frame check, and the `kdebug` GDB
harness all stay in the tree. The next stack-corruption-shaped bug
won't take two months.

## What's next

XFCE startup runs cleanly. Next: drive XFCE on Alpine to a
graphical display — either with the standard Xorg or with our own
Rust X server (kxserver, blog 174-ish). The kernel side is no
longer the limiting factor.
