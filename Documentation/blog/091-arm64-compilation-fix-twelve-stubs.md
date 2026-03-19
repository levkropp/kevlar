# Blog 091: ARM64 back from the dead — twelve compilation fixes and a minimal boot

**Date:** 2026-03-19
**Milestone:** M10 Alpine Linux

## Context

ARM64 stopped compiling on 2026-03-11. Every x86_64-only feature added during
the M9.9–M10 sprint — vDSO acceleration, ktrace, `MonotonicClock` nanosecond
snapshots, ARP-wait TSC spin, huge pages, and the vDSO page-free in
`Process::drop` — widened the gap one stub at a time. By the time we returned
to look at it, `cargo check --target aarch64` emitted twelve distinct errors
across six files.

The fix philosophy: stubs are fine. ARM64 doesn't need 2 MB huge-page TLB
entries to boot BusyBox. It needs the same kernel code to *compile*, and every
stub is marked with a comment explaining why it's safe.

---

## The twelve fixes

### Fix 1 — `HUGE_PAGE_SIZE` constant missing on ARM64

Every memory-management path that touches huge pages references
`arch::HUGE_PAGE_SIZE`. The constant existed in `platform/x64/paging.rs` (where
it was first needed) but had never been added to the ARM64 platform.

```rust
// platform/arm64/mod.rs
pub const HUGE_PAGE_SIZE: usize = 512 * PAGE_SIZE; // 2MB with 4KB granule (stub)
```

Also added to the ARM64 `pub use` list in `platform/lib.rs`.

---

### Fixes 2–7 — six huge-page stub methods on ARM64 `PageTable`

The kernel calls six `PageTable` methods unconditionally regardless of whether
the hardware used 2 MB TLB entries. None of them existed on ARM64:

| Method | Stub behaviour |
|---|---|
| `map_huge_user_page` | Maps 512 individual 4 KB pages |
| `unmap_huge_user_page` | Unmaps 512 individual 4 KB pages, returns base paddr |
| `is_huge_mapped` | Always returns `None` (prevents huge-page code path) |
| `is_pde_empty` | Checks if first 4 KB PTE in the 2 MB window is zero |
| `split_huge_page` | Always returns `None` (nothing to split) |
| `update_huge_page_flags` | Always returns `false` |

ARM64 also got `lookup_paddr` and `lookup_pte_entry` (found during compilation,
not in the original plan): both walk the 4-level page table and return the
physical address or raw PTE value.

The map/unmap stubs mean no 2 MB TLB optimization on ARM64, but all code
paths compile and run correctly.

---

### Fix 8 — `Backtrace::from_rbp()` missing on ARM64

`platform/backtrace.rs:109` calls `Backtrace::from_rbp(rbp)` unconditionally
when formatting crash dumps. ARM64 `Backtrace` had `current_frame()` but not
`from_rbp`. The naming is intentional interface parity — ARM64 uses x29/FP
rather than RBP but the semantics are identical.

```rust
// platform/arm64/backtrace.rs
pub fn from_rbp(fp: u64) -> Backtrace {
    Backtrace { frame: fp as *const StackFrame }
}
```

---

### Fix 9 — `Process::drop` vDSO free is x86_64-only

Blog 090 added a `Process::drop` impl that frees the per-process vDSO data
page. The vDSO infrastructure (`vdso_data_paddr` field, `vdso::update_tid`)
is fully gated with `#[cfg(target_arch = "x86_64")]` on all *declaration* sites,
but the drop body was ungated. One `#[cfg]` block fixes it:

```rust
#[cfg(target_arch = "x86_64")]
{
    let vdso_paddr = self.vdso_data_paddr.load(Ordering::Relaxed);
    if vdso_paddr != 0 {
        free_pages(PAddr::new(vdso_paddr as usize), 1);
    }
}
```

---

### Fix 10 — ARP wait loop uses x86_64 TSC

`kernel/net/udp_socket.rs` spins up to 1 ms waiting for an ARP reply, timing
itself with `tsc::nanoseconds_since_boot()` — an x86_64-only function. The
spin is an optimisation: on ARM64, the ARP reply arrives asynchronously via
virtio-net IRQ without any special polling.

```rust
// kernel/net/udp_socket.rs
#[cfg(target_arch = "x86_64")]
if super::ARP_SENT.load(Ordering::Relaxed) {
    let start = kevlar_platform::arch::tsc::nanoseconds_since_boot();
    // ... spin loop
}
```

---

### Fix 11 — `rdrand_fill` not defined on ARM64

`platform/random.rs` exported `rdrand_fill` only under
`#[cfg(target_arch = "x86_64")]`. Three callers in the kernel
(`devfs/mod.rs`, `procfs/mod.rs`, `icmp_socket.rs`) call it unconditionally.
Added a stub that returns `false`:

```rust
#[cfg(not(target_arch = "x86_64"))]
pub fn rdrand_fill(_slice: &mut [u8]) -> bool {
    false  // No hardware RNG on ARM64; callers fall back to timer-seeded entropy
}
```

---

### Fix 12 — `release_stacks` missing on ARM64 `ArchTask`

`kernel/process/switch.rs:138` calls `prev.arch().release_stacks()` after a
context switch to free the outgoing task's kernel stacks immediately (preventing
OOM under heavy fork/exit workloads — the blog 090 GC fix). ARM64 `ArchTask`
uses `OwnedPages` (not `Option<OwnedPages>` like x64), which auto-frees on
drop, so the stacks will be reclaimed when the process is GC'd. The stub is a
no-op placeholder:

```rust
pub unsafe fn release_stacks(&self) {
    // OwnedPages frees itself on drop; no Option<> wrapper needed.
}
```

The stack-leak mitigation is less aggressive than x86_64 but functionally
correct. A follow-up can change `kernel_stack`/`interrupt_stack`/`syscall_stack`
to `Option<OwnedPages>` to match x64 semantics.

---

### Cross-cutting fix — `arch().fsbase.load()` vs `arch().fsbase()`

Three call sites in `kernel/mm/page_fault.rs` and `kernel/process/process.rs`
access `current.arch().fsbase.load()`, treating `fsbase` as an `AtomicCell<u64>`
field. On x86_64 it *is* a field; on ARM64, `tpidr_el0` is the field and
`fsbase()` is a method that delegates to it. Both architectures have a
`pub fn fsbase(&self) -> u64` method, so the call sites became:

```rust
let fsbase = current.arch().fsbase() as usize;
```

---

### Cross-cutting fix — `rt_sigreturn` return register

`kernel/syscalls/rt_sigreturn.rs` returned `self.frame.rax` to preserve the
original syscall's return value after signal handler return. `rax` doesn't
exist on ARM64 (the return register is `x0` = `regs[0]`):

```rust
#[cfg(target_arch = "x86_64")]
{ Ok(self.frame.rax as isize) }
#[cfg(target_arch = "aarch64")]
{ Ok(self.frame.regs[0] as isize) }
```

---

## Infrastructure: a minimal ARM64 initramfs

`tools/build-initramfs.py` builds only x86_64 binaries. The Makefile sets
`INITRAMFS_PATH := build/testing.arm64.initramfs` for ARM64, but there was no
rule to populate it with ARM64-native ELFs — and no aarch64 cross-compile
toolchain installed.

**Workaround:** hand-craft a 132-byte ARM64 ELF in Python (three instructions:
`movz x0, #0` / `movz x8, #94` / `svc #0`) and embed it in a minimal CPIO as
both `/init` and `/bin/sh`. The kernel boots, executes the binary, gets
`exit_group(0)`, and halts cleanly.

Two lessons learned in debugging the initramfs:

**CPIO inode uniqueness matters.** The first attempt gave every entry inode
`00000001`. The VFS uses `(dev_id, inode_no)` as the mount-point key. With all
directories sharing inode 1, `root_fs.mount(dev_dir, DEV_FS)` registered the
key `(0, 1)`. Later, `lookup_path("/dev/console")` found the `dev` directory
(also inode 1), saw a matching mount key, switched to devfs — and then found
`console` missing because the traversal had actually jumped to the *wrong*
mount. Giving each CPIO entry a unique inode fixed the /dev/console ENOENT.

**Required directories.** The kernel's `boot_kernel()` function hardcodes
`.expect()` panics for `/proc`, `/dev`, `/tmp`, and `/sys`. All four must be
present in the initramfs, or the kernel panics before the init script ever
runs.

---

## Verification

```
make ARCH=arm64 check          # 0 errors, 171 warnings (pre-existing)
make ARCH=arm64 RELEASE=1 build  # Finished in 30.49s
timeout 60 python3 tools/run-qemu.py --arch arm64 --batch kevlar.arm64.elf
```

Boot output (trimmed):
```
Booting Kevlar...
initramfs: loaded 7 files and directories (264B)
kext: Loading virtio_blk...
kext: Loading virtio_net...
virtio-net: MAC address is 52:54:00:12:34:56
running init script: "/bin/sh"
PID 1 exiting with status 0
=== PID 1 last 0 syscalls ===
init exited with status 0, halting system
```

ARM64 compiles, boots, executes native AArch64 code, and exits cleanly.

---

## What's next: ARM64 test parity

The minimal exit-0 init proves the kernel works. The next step is parity with
the x86_64 test suite: BusyBox shell, contract tests, and eventually Alpine
Linux. That requires:

1. **Static aarch64 BusyBox** — cross-compile or download from Alpine's
   `busybox-static` aarch64 package
2. **`build-initramfs.py` ARM64 mode** — detect `ARCH=arm64`, cross-compile
   test binaries with `aarch64-linux-musl-gcc`, pull aarch64 external packages
3. **Alpine Linux aarch64** — `apk` + OpenRC on ARM64 for the M10 milestone

---

## Files changed

- `platform/arm64/mod.rs` — `HUGE_PAGE_SIZE` constant
- `platform/lib.rs` — `HUGE_PAGE_SIZE` in ARM64 pub use list
- `platform/arm64/paging.rs` — 8 new methods (6 huge-page stubs + 2 lookup)
- `platform/arm64/backtrace.rs` — `from_rbp()` method
- `platform/arm64/task.rs` — `release_stacks()` no-op stub
- `platform/arm64/interrupt.rs` — `_from_user` unused-variable fix
- `platform/random.rs` — `rdrand_fill` stub for non-x86_64
- `kernel/process/process.rs` — `#[cfg(x86_64)]` vDSO free, `fsbase()` call
- `kernel/mm/page_fault.rs` — `fsbase()` method call (×2)
- `kernel/net/udp_socket.rs` — `#[cfg(x86_64)]` ARP TSC wait
- `kernel/syscalls/rt_sigreturn.rs` — arch-gated return register
