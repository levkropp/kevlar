// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! ARM64 architecture glue — re-exports from the platform crate.

// Re-export the platform's ArchTask as Process for kernel compatibility.
pub use kevlar_platform::arch::arm64_specific::ArchTask as Process;
pub use kevlar_platform::arch::arm64_specific::switch_task as switch_thread;
pub use kevlar_platform::arch::arm64_specific::{
    USER_STACK_TOP, USER_VALLOC_BASE, USER_VALLOC_END,
};

/// ARM64 equivalent of arch_prctl — sets TLS base (TPIDR_EL0).
pub fn arch_prctl(
    current: &alloc::sync::Arc<crate::process::Process>,
    code: i32,
    uaddr: kevlar_platform::address::UserVAddr,
) -> crate::result::Result<()> {
    const ARCH_SET_FS: i32 = 0x1002;
    match code {
        ARCH_SET_FS => {
            let value = uaddr.value() as u64;
            current.arch().tpidr_el0.store(value);
            kevlar_platform::arch::arm64_specific::write_tls_base(value);
        }
        _ => {
            return Err(crate::result::Errno::EINVAL.into());
        }
    }
    Ok(())
}
