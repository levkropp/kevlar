// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#[cfg(target_arch = "x86_64")]
#[macro_use]
pub mod x64;
#[cfg(target_arch = "x86_64")]
pub use x64::*;

#[cfg(target_arch = "aarch64")]
pub mod arm64;
#[cfg(target_arch = "aarch64")]
pub use arm64::*;
