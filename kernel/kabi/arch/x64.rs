// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! x86_64 ELF relocation handlers — stubbed for K1 (arm64-only).
use crate::result::Result;

pub fn apply(_r_type: u32, _target: usize, _sym_va: usize, _addend: i64) -> Result<()> {
    unimplemented!("kabi: K1 is arm64-only; x86_64 module loading is K2 work");
}
