// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::prelude::*;
use goblin::elf64::header::{Header, ELFMAG, ET_EXEC, ET_DYN};

#[cfg(target_arch = "x86_64")]
use goblin::elf64::header::EM_X86_64;

#[cfg(target_arch = "x86_64")]
const EXPECTED_MACHINE: u16 = EM_X86_64;
#[cfg(target_arch = "aarch64")]
const EXPECTED_MACHINE: u16 = 183; // EM_AARCH64
pub use goblin::elf64::program_header::ProgramHeader;
pub const PT_INTERP: u32 = 3;
use kevlar_platform::address::UserVAddr;

/// A parsed ELF object.
pub struct Elf<'a> {
    header: &'a Header,
    program_headers: &'a [ProgramHeader],
}

impl<'a> Elf<'a> {
    /// Parses a ELF header. Accepts both ET_EXEC and ET_DYN (PIE / shared objects).
    pub fn parse(buf: &'a [u8]) -> Result<Elf<'a>> {
        if buf.len() < size_of::<Header>() {
            debug_warn!("ELF header buffer is too short");
            return Err(Errno::ENOEXEC.into());
        }

        let header: &Header = kevlar_platform::pod::ref_from_prefix(buf)
            .ok_or_else(|| Error::new(Errno::ENOEXEC))?;
        if &header.e_ident[..4] != ELFMAG {
            debug_warn!("invalid ELF magic");
            return Err(Errno::ENOEXEC.into());
        }

        if header.e_machine != EXPECTED_MACHINE {
            debug_warn!("invalid ELF e_machine");
            return Err(Errno::ENOEXEC.into());
        }

        if header.e_type != ET_EXEC && header.e_type != ET_DYN {
            debug_warn!("ELF is not executable or shared object (e_type={})", header.e_type);
            return Err(Errno::ENOEXEC.into());
        }

        let ph_offset = header.e_phoff as usize;
        let ph_count = header.e_phnum as usize;
        let program_headers = kevlar_platform::pod::slice_from_prefix(
            &buf[ph_offset..],
            ph_count,
        ).ok_or_else(|| Error::new(Errno::ENOEXEC))?;

        Ok(Elf {
            header,
            program_headers,
        })
    }

    /// Returns true if this is a position-independent executable (ET_DYN).
    pub fn is_dyn(&self) -> bool {
        self.header.e_type == ET_DYN
    }

    /// The raw entry point offset from the ELF header.
    pub fn entry_offset(&self) -> u64 {
        self.header.e_entry
    }

    /// The entry point of the ELF file (for ET_EXEC with fixed addresses).
    pub fn entry(&self) -> Result<UserVAddr> {
        UserVAddr::new_nonnull(self.header.e_entry as usize).map_err(Into::into)
    }

    /// The ELF header.
    pub fn header(&self) -> &Header {
        self.header
    }

    /// Program headers.
    pub fn program_headers(&self) -> &[ProgramHeader] {
        self.program_headers
    }

    /// Find PT_INTERP and return the interpreter path from the buffer.
    pub fn interp_path<'b>(&self, buf: &'b [u8]) -> Option<&'b str> {
        for phdr in self.program_headers {
            if phdr.p_type == PT_INTERP {
                let start = phdr.p_offset as usize;
                let end = start + phdr.p_filesz as usize;
                if end <= buf.len() {
                    // Strip trailing NUL.
                    let slice = &buf[start..end];
                    let len = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
                    return core::str::from_utf8(&slice[..len]).ok();
                }
            }
        }
        None
    }
}
