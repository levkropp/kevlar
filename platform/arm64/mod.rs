// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use core::arch::global_asm;

global_asm!(include_str!("boot.S"));
global_asm!(include_str!("trap.S"));
global_asm!(include_str!("usercopy.S"));
global_asm!(include_str!("usermode.S"));

#[macro_use]
mod cpu_local;

mod backtrace;
mod boot;
mod bootinfo;
mod gic;
mod idle;
mod interrupt;
mod paging;
mod profile;
mod semihosting;
mod serial;
mod syscall;
pub mod task;
mod timer;

pub use backtrace::Backtrace;
pub use idle::{halt, idle};
pub use interrupt::SavedInterruptStatus;
pub use gic::enable_irq;
pub use paging::{PageFaultReason, PageTable};
pub use profile::read_clock_counter;
pub use semihosting::{semihosting_halt, SemihostingExitStatus};
pub use syscall::PtRegs;

pub mod arm64_specific {
    pub use super::cpu_local::cpu_local_head;
    pub use super::task::{
        ArchTask, switch_task, write_tls_base,
        KERNEL_STACK_SIZE, USER_VALLOC_END, USER_VALLOC_BASE, USER_STACK_TOP,
    };
}

/// ARM64 SMP not yet implemented; always returns 1.
pub fn num_online_cpus() -> u32 {
    1
}

pub const PAGE_SIZE: usize = 4096;
pub const TICK_HZ: usize = 50;

/// The base virtual address of straight mapping (TTBR1 region).
pub const KERNEL_BASE_ADDR: usize = 0xffff_0000_0000_0000;

/// The end of straight mapping. Any physical address `P` is mapped into the
/// kernel's virtual memory address `KERNEL_BASE_ADDR + P`.
pub const KERNEL_STRAIGHT_MAP_PADDR_END: usize = 0x1_0000_0000;
