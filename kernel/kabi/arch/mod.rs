// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Per-arch relocation handlers.
#[cfg(target_arch = "aarch64")]
pub mod arm64;
#[cfg(target_arch = "x86_64")]
pub mod x64;
