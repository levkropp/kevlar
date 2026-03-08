# Milestone 1.5: ARM64 BusyBox Boots on Kevlar

**Date:** 2026-03-08

---

Kevlar now runs on ARM64. The same static musl BusyBox that boots on x86_64 boots on QEMU's `virt` machine (cortex-a72), producing an interactive shell. This is the first multi-architecture milestone: one kernel codebase, two ISAs, same Linux userspace.

```
Kevlar ARM64 booting...
bootinfo OK, ram_areas=1
page_allocator OK
...
/ # echo hello from arm64
hello from arm64
/ # ls /
bin                integration_tests  sys                var
dev                proc               tmp
etc                sbin               usr
```

## What it took

### New platform code

ARM64 required a complete HAL layer under `runtime/arm64/`:

- **boot.S** (~200 lines) -- EL2-to-EL1 drop, MMU setup with identity + high-half mappings, BSS clear, stack init, branch to Rust
- **paging.rs** -- 4-level page table manipulation using ARMv8-A descriptors (4KB granule, TTBR0 for user, TTBR1 for kernel)
- **trap.S** -- Full VBAR_EL1 vector table: synchronous, IRQ, FIQ, SError, from both EL1 and EL0
- **interrupt.rs** -- Exception dispatch: SVC for syscalls, data/instruction aborts for page faults, IRQ routing
- **gic.rs** -- GICv2 distributor + CPU interface (QEMU virt uses GICv2 at 0x08000000)
- **timer.rs** -- ARM generic timer (CNTP) for preemptive scheduling
- **serial.rs** -- PL011 UART for console I/O
- **bootinfo.rs** -- Minimal DTB parser for memory regions and command line
- **usercopy.S** -- Fault-tolerant user-space memory access (copy_from_user, copy_to_user, strncpy_from_user)
- **syscall.rs** -- Linux AArch64 syscall ABI (x8 = syscall number, x0-x5 = args)
- **semihosting.rs** -- ARM semihosting for clean QEMU exit

The kernel-side changes were minimal: a new `kernel/arch/arm64/` with process context switching (callee-saved registers + SP + PC) and the syscall number mapping table.

### Three bugs that mattered

**AP bit encoding.** ARM64 page table Access Permission bits are AP[2:1] at bits [7:6]. The initial implementation had AP_USER and AP_RO swapped, meaning user pages were mapped as EL1-read-only with no EL0 access. Every user-space memory access faulted immediately. The fix:

```rust
// Before (wrong): AP_USER at bit 7, AP_RO at bit 6
const ATTR_AP_RO: u64 = 1 << 6;
const ATTR_AP_USER: u64 = 1 << 7;

// After (correct): AP[1]=EL0 access at bit 6, AP[2]=read-only at bit 7
const ATTR_AP_USER: u64 = 1 << 6;
const ATTR_AP_RO: u64 = 1 << 7;
```

Two constants, swapped. Hours of debugging exception loops.

**WFI trapping.** SCTLR_EL1.nTWI (bit 16) was not set, causing `WFI` in the idle loop to trap as an undefined instruction instead of waiting for an interrupt. Added `orr x0, x0, #(1 << 16)` to the SCTLR_EL1 setup in boot.S.

**Usercopy fault detection.** On x86_64, the kernel page fault handler checks if the faulting PC matches specific usercopy instruction addresses. On ARM64, the equivalent approach uses a range check: all usercopy functions are placed between `usercopy_start` and `usercopy_end` labels, and faults within that range are handled gracefully instead of panicking.

### QEMU TCG performance

ARM64 debug builds are too slow under QEMU's TCG (software emulation). Operations like `ArrayVec::new()` that compile to a few instructions in release mode expand to large memset calls with bounds checking in debug mode. Under TCG, these cause the boot sequence to hang for minutes. All ARM64 testing uses `RELEASE=1`.

### What didn't need to change

The entire kernel above the HAL layer -- syscall implementations, VFS, process management, networking, memory management -- runs unmodified on ARM64. The `runtime` crate's architecture abstraction held up: the kernel calls `handler().handle_page_fault()`, `handler().handle_irq()`, `handler().handle_timer_irq()`, and the platform delivers. 79 syscalls, zero kernel-side changes for ARM64.

## Building and running

```bash
# ARM64 (release required for TCG performance)
RELEASE=1 ARCH=arm64 make run

# x86_64 (unchanged)
make run
```

ARM64 uses QEMU's `virt` machine with cortex-a72, GICv2, PL011 UART, and virtio-mmio. No PCI -- the virt machine uses 32 virtio-mmio transport slots at 0x0a000000.

## What's next

**M2: Dynamic linking.** The milestone after this is making dynamically linked musl binaries work. `ld-linux-aarch64.so` and `ld-linux-x86-64.so` need `pread64`, `futex`, `madvise`, and `mremap`. Both architectures will be tested in parallel from here forward.

The ARM64 port validates the architecture split. Adding a new platform is a bounded amount of work -- roughly 1500 lines of new code -- and the kernel's syscall coverage carries over automatically. The next architecture (RISC-V, whenever that happens) should be even easier.
