// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Arch-neutral relocation dispatcher.

use crate::result::Result;

/// Apply one relocation entry.
///
/// `target` — the VA in the loaded module image where the relocation
///   is applied (== section base + Rela::r_offset).
/// `sym_va` — the resolved address of the relocation's symbol.
/// `addend` — the relocation's r_addend.
/// `r_type` — the architecture's relocation type number.
#[inline]
pub fn apply(r_type: u32, target: usize, sym_va: usize, addend: i64) -> Result<()> {
    #[cfg(target_arch = "aarch64")]
    return crate::kabi::arch::arm64::apply(r_type, target, sym_va, addend);
    #[cfg(target_arch = "x86_64")]
    return crate::kabi::arch::x64::apply(r_type, target, sym_va, addend);
}
