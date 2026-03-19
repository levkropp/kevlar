# Blog 093: ARM64 contract tests — from 0/118 to 89/118 in one session

**Date:** 2026-03-19
**Milestone:** M10 Alpine Linux

## Context

ARM64 BusyBox booted (Blog 091) and ktrace was ported (Blog 092), but the
contract test suite — 118 behavioral tests that compare Kevlar's syscall
output to Linux — had never been run on ARM64.  The first run: **0/118 PASS**.
Every test either panicked the kernel, got the wrong binary, or produced wrong
output.  Six distinct categories of bugs were responsible.

---

## Bug 1: KEVLAR_INIT patchable slot (0 → all tests reachable)

**Problem:** `compare-contracts.py` tells Kevlar which contract binary to run
via `init=/bin/contract-foo` on the kernel cmdline.  On x86_64, QEMU's
multiboot loader passes the cmdline string through the boot info struct.
On ARM64, QEMU does *not* pass a DTB (or cmdline) when loading a bare-metal
ELF kernel — the ARM Linux boot protocol only applies to `Image`-format
kernels.  Every test was running `/sbin/init` (the default), not the contract
binary.

**Fix:** A 128-byte `#[used] #[unsafe(link_section = ".rodata")]` static
buffer with a magic prefix `KEVLAR_INIT:` that `compare-contracts.py` binary-
patches in the ELF before each test run:

```rust
static INIT_SLOT: [u8; 128] = {
    let mut buf = [0u8; 128];
    buf[0] = b'K'; buf[1] = b'E'; /* ... */ buf[11] = b':';
    buf
};
```

The kernel reads it with volatile loads at boot (to defeat constant folding)
and uses the patched path as `argv[0]`.  The Python side finds the magic bytes
via `elf_data.find(b"KEVLAR_INIT:")` and overwrites the payload region.

This mechanism works on both architectures — x86_64 still has the cmdline as a
fallback, but now also gets the slot patch for consistency.

## Bug 2: ARM64 stat struct ABI (5 tests fixed)

**Tests:** `fchmod_accept`, `link_hardlink`, `statx_fields`, `symlink_readlink`,
`mkdir_rmdir`

**Problem:** The stat syscalls (`fstat`, `lstat`, `stat`, `newfstatat`) were
writing Kevlar's internal `Stat` struct directly to userspace via
`buf.write(&stat)`.  The internal struct matches x86_64's layout:

```
offset 16: st_nlink (u64)
offset 24: st_mode  (u32)
```

But ARM64's `asm-generic/stat.h` layout is:

```
offset 16: st_mode  (u32)
offset 20: st_nlink (u32)   ← 32-bit, not 64-bit!
```

The test binaries (compiled with musl for aarch64) read `st_mode` from offset
16 and got `st_nlink`'s value instead.  A regular file showed `mode=0x1`
(nlink=1 misread as mode) instead of `0x8180` (S_IFREG|0600).

**Fix:** Added `Stat::to_abi_bytes()` with `#[cfg(target_arch)]` variants:

- **ARM64:** manually serializes `mode(u32)|nlink(u32)` at offset 16,
  `blksize(i32)` at offset 56, returns `[u8; 128]`
- **x86_64:** `memcpy` of the struct (already matches), returns `[u8; 144]`

All four stat syscalls now call `buf.write(&stat.to_abi_bytes())`.

## Bug 3: ARM64 syscall number mismatches (6 syscalls fixed)

**Tests:** `fchmod_accept`, `fchown_accept`, `sched_getscheduler_accept`, plus
indirect failures from wrong dispatch

ARM64 uses the `asm-generic/unistd.h` numbering which differs significantly
from x86_64.  Six constants were wrong:

| Syscall             | Wrong | Correct |
|---------------------|-------|---------|
| SYS_FCHMOD          | 0xF010 (stub) | 52  |
| SYS_FCHOWN          | 0xF011 (stub) | 55  |
| SYS_FCHOWNAT        | 55    | 54      |
| SYS_SCHED_GETSCHEDULER | 121 | 120   |
| SYS_VHANGUP         | (missing) | 58  |
| SYS_PSELECT6        | (missing) | 72  |

`FCHMOD` and `FCHOWN` were deliberately set to impossible values (`0xF0xx`)
under the assumption that ARM64 only has `fchmodat`/`fchownat`.  In reality,
ARM64's asm-generic ABI *does* include the non-at variants.

## Bug 4: ARM64 signal delivery (signal path enabled)

**Problem:** After a syscall returns from user-space (`svc #0`), the kernel
must check for pending signals before `eret`-ing back.  On x86_64 this is
`x64_check_signal_on_irq_return` called from the IRET path.  ARM64 had no
equivalent — the `handle_lower_a64_sync` and `handle_lower_a64_irq` paths in
`trap.S` went straight from the Rust handler to `RESTORE_REGS + eret`.

**Fix:** Added `arm64_check_signal_on_return(frame)` in `interrupt.rs`,
called from both lower-EL return paths in `trap.S`:

```asm
handle_lower_a64_sync:
    SAVE_REGS
    mov     x0, #1
    mov     x1, sp
    bl      arm64_handle_exception
+   mov     x0, sp
+   bl      arm64_check_signal_on_return
    RESTORE_REGS
    eret
```

The Rust function mirrors x64: check `signal_pending` atomic, if non-zero call
`handle_interrupt_return` which pops the signal and calls `setup_signal_stack`
to redirect ELR_EL1 to the handler.

## Bug 5: PROT_NONE must not set AP_USER (PROT_NONE fix)

**Test:** `mprotect_guard_segv`

**Problem:** ARM64's `prot_to_attrs()` unconditionally set `ATTR_AP_USER`
(AP[1]=1), making every page accessible from EL0.  A `PROT_NONE` mapping
should be completely inaccessible, but the AP bit made it readable.

**Fix:** Only set `ATTR_AP_USER` when `prot_flags & 3 != 0` (PROT_READ or
PROT_WRITE).  For `PROT_NONE`, AP[1] stays 0 so EL0 access triggers a
permission fault → SIGSEGV.

## Bug 6: Boot and test harness fixes

**Default boot info:** Bumped from 256MB to 1GB (`-m 1024`) to match the
contract test QEMU invocation.  Removed virtio-mmio probing from
`default_boot_info()` — each of the 32 probes takes ~1.5s under TCG
(48 seconds total, exceeding the 30-second test timeout).

**DTB scan:** Simplified — QEMU doesn't place a DTB in guest RAM for ELF
kernels, so `scan_for_dtb()` always returns `None`.  Kept as a fallback but
removed the log spam.

**Noise filtering:** `compare-contracts.py` now strips ARM64 boot messages
(RAM info, page allocator, DTB status) that would otherwise cause spurious
DIVG results.

**pselect6:** Added dispatch for `SYS_PSELECT6` (ARM64 nr 72), converting
the `struct timespec` argument to `Timeval` and delegating to `sys_select`.

---

## Results

| Arch   | Before | After  | Delta |
|--------|--------|--------|-------|
| ARM64  | 0/118  | 89/118 | +89   |
| x86_64 | (unchanged) | (unchanged) | —     |

**Remaining 29 ARM64 failures:**

- **Signal delivery (5):** `alarm_delivery`, `handler_context`, `sa_restart`,
  `setitimer_oneshot`, `sigsuspend_wake` — signal handler never invoked
  (`received=0`).  The trap.S plumbing is in place but the handler isn't
  firing; investigation ongoing.
- **Filesystem (4):** stat-related tests that may need additional ABI fixes.
- **VM (2):** `mmap_shared` (shared mapping not visible), `mremap_grow`
  (sentinel corrupted after grow).
- **Subsystems (1):** `proc_global` (missing ARM64 cpuinfo field).
- **Timeouts (2):** `poll_basic`, `inotify_create_xfail` (30s timeout, both
  arches).
- **Both-arch issues (15):** mmap address format, setsockopt values, getrusage
  utime, socket panics, various divergences shared with x86_64.

## What's next

Signal delivery is the highest-value target — fixing it would flip 5 tests and
validate the entire ARM64 signal path (trampoline, sigreturn, signal frame
save/restore).  The `setup_signal_stack` code in `platform/arm64/task.rs`
looks correct, so the bug is likely in how `signal_pending` is checked or how
the signal action is resolved for the self-signal case (`kill(getpid(), sig)`).
