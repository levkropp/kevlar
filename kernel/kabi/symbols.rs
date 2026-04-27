// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Symbol resolution for loaded `.ko` modules.
//!
//! Two flavors of symbol resolution:
//!
//! - **Module-internal**: the module's own .symtab references its own
//!   sections (e.g. `my_init` is defined in its `.text`).  Resolved to
//!   `section_va_map[shndx] + sym.st_value`.
//!
//! - **External (kernel-exported)**: an undefined symbol whose name
//!   matches an entry in `kabi::exports::all()`.  Resolved by linear
//!   lookup against the linker-collected `.ksymtab` table.

use alloc::vec::Vec;

use crate::kabi::exports;
use crate::prelude::*;

/// `SHN_*` constants from ELF.
pub const SHN_UNDEF: u16 = 0;
pub const SHN_ABS: u16 = 0xfff1;
pub const SHN_COMMON: u16 = 0xfff2;

/// Resolve one symbol — either internally to the module's own
/// sections, or externally to the kernel's exported symbol table.
pub fn resolve(
    sym_shndx: u16,
    sym_value: u64,
    sym_name: &str,
    section_va_map: &Vec<Option<usize>>,
) -> Result<usize> {
    match sym_shndx {
        SHN_UNDEF => {
            // External symbol — look up in kernel exports.
            exports::lookup(sym_name).ok_or_else(|| {
                log::warn!(
                    "kabi: undefined external symbol '{}' — not in kernel exports table",
                    sym_name
                );
                Errno::ENOENT.into()
            })
        }
        SHN_ABS => {
            // Absolute symbol — just the st_value.
            Ok(sym_value as usize)
        }
        SHN_COMMON => {
            // Common (uninitialized BSS-style globals).  Real
            // module loaders allocate these from a separate region.
            // K1 hello-world doesn't emit these.
            panic!(
                "kabi: K1 doesn't support SHN_COMMON symbols (got '{}')",
                sym_name
            );
        }
        idx if (idx as usize) < section_va_map.len() => {
            // Defined in one of the module's own sections.
            match section_va_map[idx as usize] {
                Some(va) => Ok(va + sym_value as usize),
                None => {
                    log::warn!(
                        "kabi: symbol '{}' in section {} which wasn't loaded",
                        sym_name, idx
                    );
                    Err(Errno::ENOEXEC.into())
                }
            }
        }
        _ => {
            log::warn!(
                "kabi: symbol '{}' has out-of-range section index {}",
                sym_name, sym_shndx
            );
            Err(Errno::ENOEXEC.into())
        }
    }
}
