// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Kevlar API for kernel extensions.
#![no_std]

extern crate alloc;

#[macro_use]
extern crate log;

pub mod driver;
pub mod kernel_ops;
pub mod net;

pub use kevlar_platform::{debug_warn, warn_if_err, warn_once};
pub use log::{debug, error, info, trace, warn};

pub mod address {
    pub use kevlar_platform::address::{PAddr, VAddr};
}

pub mod mm {
    pub use kevlar_platform::page_allocator::{alloc_pages, AllocPageFlags, PageAllocError};
}

pub mod sync {
    pub use kevlar_platform::spinlock::{SpinLock, SpinLockGuard};
}

pub mod arch {
    pub use kevlar_platform::arch::PAGE_SIZE;
}
