// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! x86_64 architecture glue — re-exports from the platform crate.

mod arch_prctl;

pub use arch_prctl::arch_prctl;

// Re-export the platform's ArchTask as Process for kernel compatibility.
pub use kevlar_platform::arch::x64_specific::ArchTask as Process;
pub use kevlar_platform::arch::x64_specific::switch_task as switch_thread;
pub use kevlar_platform::arch::x64_specific::{
    USER_STACK_TOP, USER_VALLOC_BASE, USER_VALLOC_END,
};
