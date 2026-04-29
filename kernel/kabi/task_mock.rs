// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Per-CPU Linux task_struct mock pointed at by `sp_el0`.
//!
//! Linux 7.0 arm64 stores the running `task_struct *` in `sp_el0`.
//! Functions read it via `mrs xN, sp_el0` then dereference fields
//! at fixed offsets — e.g. `current->stack_canary` at +1912 in
//! `erofs_read_superblock` for the stack-protect prologue.
//!
//! Kevlar's runtime contract uses `sp_el0` differently: it holds
//! the user stack pointer, saved/restored on EL0↔EL1 transitions
//! via `PtRegs[31]`.  When a kABI-loaded `.ko` runs in kernel
//! mode and reads `sp_el0`, it gets whatever was last there —
//! typically a user SP, which doesn't have a Linux task_struct
//! at the dereferenced offsets.
//!
//! This module bridges the two: a per-CPU mock buffer with
//! Linux 7.0 task_struct field offsets populated for the few
//! fields fs code actually reads.  `init_per_cpu_mocks` allocates
//! the buffers + writes the sentinel values.  `install_for_current_cpu`
//! sets the running CPU's `sp_el0` to its mock.  `trap.S` does the
//! same on every EL0→EL1 transition so the mock stays in place
//! even after a user-mode trap saved the user SP.

use core::cell::UnsafeCell;

/// Maximum number of CPUs we provision a mock for.  Matches the
/// upper bound used elsewhere in Kevlar.
pub const MAX_CPUS: usize = 16;

/// Mock buffer size.  Linux 7.0 task_struct is < 8 KiB; 4 KiB
/// is enough for every field fs `.ko` code is observed to read
/// during mount.  Aligned to page boundary for cache friendliness.
const MOCK_SIZE: usize = 4096;

/// Linux 7.0 arm64 `task_struct.stack_canary` offset, observed
/// via `erofs_read_superblock` disasm at offset 0x43dc:
///
///     mrs x0, sp_el0
///     ldr x1, [x0, #1912]
///
/// Any non-zero value satisfies the stack-protect epilogue's
/// `cmp x1, x_saved`.  This sentinel is recognisable in
/// post-mortem dumps.
pub const STACK_CANARY_OFF: usize = 1912;
const STACK_CANARY_SENTINEL: u64 = 0xDEAD_DEAD_BEEF_BEEF;

#[repr(C, align(4096))]
struct AlignedMock(UnsafeCell<[u8; MOCK_SIZE]>);

// SAFETY: each CPU touches its own slot only.  The cross-CPU
// contract is "no concurrent writes after init"; init is single-
// threaded on the BSP.  Reads after init are immutable.
unsafe impl Sync for AlignedMock {}

/// Per-CPU mock storage.  Lives in `.bss`; written once at
/// `init_per_cpu_mocks()` and then read-only thereafter (Linux fs
/// code only ever reads from `current->...` during mount paths
/// we exercise).
static MOCKS: [AlignedMock; MAX_CPUS] =
    [const { AlignedMock(UnsafeCell::new([0; MOCK_SIZE])) }; MAX_CPUS];

/// Initialise all per-CPU mocks (write the sentinel canary).
/// Call once from `kabi::init()` on the BSP after kabi alloc is up.
pub fn init_per_cpu_mocks() {
    let bytes = STACK_CANARY_SENTINEL.to_ne_bytes();
    for cpu in 0..MAX_CPUS {
        let mock_data = MOCKS[cpu].0.get();
        unsafe {
            let arr = &mut *mock_data;
            for i in 0..8 {
                arr[STACK_CANARY_OFF + i] = bytes[i];
            }
        }
    }
    log::info!(
        "kabi: task_mock initialised {} per-CPU mocks ({}KB each, \
         canary={:#x} at +{})",
        MAX_CPUS, MOCK_SIZE / 1024, STACK_CANARY_SENTINEL, STACK_CANARY_OFF,
    );
}

/// Returns the address of the per-CPU mock for CPU `cpu_id`.
/// Returns 0 if `cpu_id` is out of range.
pub fn mock_addr_for(cpu_id: usize) -> u64 {
    if cpu_id >= MAX_CPUS {
        return 0;
    }
    MOCKS[cpu_id].0.get() as u64
}

/// Install the per-CPU mock pointer in this CPU's `CpuLocalHead`
/// + write `sp_el0`.  Trap.S reads `kabi_task_mock_ptr` from
/// CpuLocalHead and `msr sp_el0, x` on every EL0→EL1 entry, so
/// this initial setup persists.
pub fn install_for_current_cpu() {
    let cpu = kevlar_platform::arch::cpu_id() as usize;
    let addr = mock_addr_for(cpu);
    if addr == 0 {
        log::warn!("kabi: task_mock install: cpu {} out of range", cpu);
        return;
    }
    let head = kevlar_platform::arch::arm64_specific::cpu_local_head();
    head.kabi_task_mock_ptr = addr;
    unsafe {
        core::arch::asm!("msr sp_el0, {}", in(reg) addr);
    }
    log::info!(
        "kabi: task_mock cpu={} sp_el0={:#x} (canary at +{} = {:#x})",
        cpu, addr, STACK_CANARY_OFF, STACK_CANARY_SENTINEL,
    );
}
