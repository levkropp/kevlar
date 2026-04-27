// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! ELF ET_REL parser for kABI module loading.
//!
//! Built on `goblin::elf64::*` POD types — same crate that
//! `kernel/process/elf.rs` uses for userspace ELF binaries.  Unlike
//! that path, this parser walks *section headers* (modules use
//! section-based layout, not program-header-based), `.symtab` /
//! `.strtab`, and `.rela.<section>` tables.

use goblin::elf64::header::{Header, ELFMAG, ET_REL};
use goblin::elf64::reloc::Rela;
use goblin::elf64::section_header::SectionHeader;
use goblin::elf64::sym::Sym;

use crate::prelude::*;

#[cfg(target_arch = "aarch64")]
const EXPECTED_MACHINE: u16 = 183; // EM_AARCH64
#[cfg(target_arch = "x86_64")]
const EXPECTED_MACHINE: u16 = 62; // EM_X86_64

// ELF section types we care about.
pub const SHT_NULL: u32 = 0;
pub const SHT_PROGBITS: u32 = 1;
pub const SHT_SYMTAB: u32 = 2;
pub const SHT_STRTAB: u32 = 3;
pub const SHT_RELA: u32 = 4;
pub const SHT_NOBITS: u32 = 8;

// ELF section flags we care about.
pub const SHF_ALLOC: u64 = 0x2;
pub const SHF_EXECINSTR: u64 = 0x4;

/// Reloc-section view: `.rela.<target>` describing fixups for the
/// `target_section` of the same module.
pub struct RelaTable<'a> {
    pub target_section: usize,
    pub entries: &'a [Rela],
}

/// A parsed ET_REL ELF object backed by a borrowed byte slice.
pub struct RelObj<'a> {
    pub buf: &'a [u8],
    pub header: &'a Header,
    pub sections: &'a [SectionHeader],
    /// Section header string table (the strings naming each section).
    pub shstrtab: &'a [u8],
    /// Index of the .symtab section.
    pub symtab_idx: usize,
    /// Symbol table entries.
    pub symtab: &'a [Sym],
    /// String table for `.symtab` (linked via `sh_link`).
    pub strtab: &'a [u8],
}

impl<'a> RelObj<'a> {
    pub fn parse(buf: &'a [u8]) -> Result<RelObj<'a>> {
        if buf.len() < size_of::<Header>() {
            debug_warn!("kabi: ELF too small");
            return Err(Errno::ENOEXEC.into());
        }

        let header: &Header = kevlar_platform::pod::ref_from_prefix(buf)
            .ok_or_else(|| Error::new(Errno::ENOEXEC))?;
        if &header.e_ident[..4] != ELFMAG {
            debug_warn!("kabi: bad ELF magic");
            return Err(Errno::ENOEXEC.into());
        }
        if header.e_type != ET_REL {
            debug_warn!("kabi: not ET_REL (e_type={})", header.e_type);
            return Err(Errno::ENOEXEC.into());
        }
        if header.e_machine != EXPECTED_MACHINE {
            debug_warn!("kabi: wrong e_machine ({})", header.e_machine);
            return Err(Errno::ENOEXEC.into());
        }

        // Section header table.
        let sh_off = header.e_shoff as usize;
        let sh_n = header.e_shnum as usize;
        if sh_n == 0 || sh_off >= buf.len() {
            debug_warn!("kabi: no section header table");
            return Err(Errno::ENOEXEC.into());
        }
        let sections: &[SectionHeader] = kevlar_platform::pod::slice_from_prefix(
            &buf[sh_off..],
            sh_n,
        )
        .ok_or_else(|| Error::new(Errno::ENOEXEC))?;

        // Section header string table — names of each section.
        let shstr_idx = header.e_shstrndx as usize;
        if shstr_idx >= sh_n {
            debug_warn!("kabi: bad e_shstrndx");
            return Err(Errno::ENOEXEC.into());
        }
        let shstr_sh = &sections[shstr_idx];
        let shstr_start = shstr_sh.sh_offset as usize;
        let shstr_end = shstr_start + shstr_sh.sh_size as usize;
        if shstr_end > buf.len() {
            debug_warn!("kabi: shstrtab out of range");
            return Err(Errno::ENOEXEC.into());
        }
        let shstrtab = &buf[shstr_start..shstr_end];

        // Find .symtab + its associated .strtab.
        let mut symtab_idx_opt: Option<usize> = None;
        for (i, sh) in sections.iter().enumerate() {
            if sh.sh_type == SHT_SYMTAB {
                symtab_idx_opt = Some(i);
                break;
            }
        }
        let symtab_idx = symtab_idx_opt.ok_or_else(|| {
            debug_warn!("kabi: no .symtab section");
            Error::new(Errno::ENOEXEC)
        })?;

        let symtab_sh = &sections[symtab_idx];
        let sym_n = (symtab_sh.sh_size as usize) / size_of::<Sym>();
        let symtab: &[Sym] = kevlar_platform::pod::slice_from_prefix(
            &buf[symtab_sh.sh_offset as usize..],
            sym_n,
        )
        .ok_or_else(|| Error::new(Errno::ENOEXEC))?;

        // sh_link of the symtab points at the strtab.
        let strtab_idx = symtab_sh.sh_link as usize;
        if strtab_idx >= sh_n {
            debug_warn!("kabi: symtab.sh_link out of range");
            return Err(Errno::ENOEXEC.into());
        }
        let strtab_sh = &sections[strtab_idx];
        let strtab_start = strtab_sh.sh_offset as usize;
        let strtab_end = strtab_start + strtab_sh.sh_size as usize;
        if strtab_end > buf.len() {
            debug_warn!("kabi: strtab out of range");
            return Err(Errno::ENOEXEC.into());
        }
        let strtab = &buf[strtab_start..strtab_end];

        Ok(RelObj {
            buf,
            header,
            sections,
            shstrtab,
            symtab_idx,
            symtab,
            strtab,
        })
    }

    /// Read a NUL-terminated string from `shstrtab` at `offset`.
    pub fn section_name(&self, sh: &SectionHeader) -> &'a str {
        read_cstr(self.shstrtab, sh.sh_name as usize).unwrap_or("")
    }

    /// Read a symbol's name from `strtab` at the given offset.
    pub fn sym_name(&self, sym: &Sym) -> &'a str {
        read_cstr(self.strtab, sym.st_name as usize).unwrap_or("")
    }

    /// Iterate over all `.rela.*` relocation tables in the object.
    pub fn rela_tables(&self) -> impl Iterator<Item = RelaTable<'a>> + '_ {
        self.sections.iter().enumerate().filter_map(move |(i, sh)| {
            if sh.sh_type != SHT_RELA {
                return None;
            }
            let n = (sh.sh_size as usize) / size_of::<Rela>();
            let entries: &[Rela] = kevlar_platform::pod::slice_from_prefix(
                &self.buf[sh.sh_offset as usize..],
                n,
            )?;
            // sh_info on a SHT_RELA points at the section the relocs apply to.
            let target_section = sh.sh_info as usize;
            // Sanity-check: target should be a section we'll actually load.
            // (We don't gate here; loader skips relocations against
            // non-loaded sections.)
            let _ = i;
            Some(RelaTable {
                target_section,
                entries,
            })
        })
    }
}

fn read_cstr(buf: &[u8], offset: usize) -> Option<&str> {
    if offset >= buf.len() {
        return None;
    }
    let slice = &buf[offset..];
    let len = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
    core::str::from_utf8(&slice[..len]).ok()
}
