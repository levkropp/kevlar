// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::result::{Errno, Error, Result};

use alloc::vec::Vec;
use kevlar_platform::address::{UserVAddr, VAddr};

use core::mem::size_of;

use kevlar_utils::alignment::align_up;

pub enum Auxv {
    /// End of a vector.
    Null,
    /// The address of the ELF program headers.
    Phdr(UserVAddr),
    /// The size of a program header.
    Phent(usize),
    /// The number of program headers.
    Phnum(usize),
    /// The size of a page.
    Pagesz(usize),
    /// Base address where the interpreter was loaded.
    Base(usize),
    /// Entry point of the main executable.
    Entry(usize),
    /// Real UID.
    Uid(usize),
    /// Effective UID.
    Euid(usize),
    /// Real GID.
    Gid(usize),
    /// Effective GID.
    Egid(usize),
    /// Whether the process is setuid/setgid.
    Secure(usize),
    /// 16 random bytes. Used for stack canary.
    Random([u8; 16]),
}

fn push_bytes_to_stack(sp: &mut VAddr, stack_bottom: VAddr, buf: &[u8]) -> Result<()> {
    if sp.sub(buf.len()) < stack_bottom {
        return Err(Error::with_message(Errno::E2BIG, "too big argvp/envp/auxv"));
    }

    *sp = sp.sub(buf.len());
    sp.write_bytes(buf);
    Ok(())
}

fn push_usize_to_stack(sp: &mut VAddr, stack_bottom: VAddr, value: usize) -> Result<()> {
    if cfg!(target_endian = "big") {
        push_bytes_to_stack(sp, stack_bottom, &value.to_be_bytes())?;
    } else {
        push_bytes_to_stack(sp, stack_bottom, &value.to_le_bytes())?;
    }

    Ok(())
}

fn push_aux_data_to_stack(sp: &mut VAddr, stack_bottom: VAddr, auxv: &Auxv) -> Result<()> {
    match auxv {
        Auxv::Random(values) => push_bytes_to_stack(sp, stack_bottom, values.as_slice())?,
        _ => {}
    }

    Ok(())
}

fn push_auxv_entry_to_stack(
    sp: &mut VAddr,
    stack_bottom: VAddr,
    auxv: &Auxv,
    data_ptr: Option<UserVAddr>,
) -> Result<()> {
    let (auxv_type, value) = match auxv {
        Auxv::Null => (0, 0),
        Auxv::Phdr(uaddr) => (3, uaddr.value()),
        Auxv::Phent(value) => (4, *value),
        Auxv::Phnum(value) => (5, *value),
        Auxv::Pagesz(value) => (6, *value),
        Auxv::Base(value) => (7, *value),
        Auxv::Entry(value) => (9, *value),
        Auxv::Uid(value) => (11, *value),
        Auxv::Euid(value) => (12, *value),
        Auxv::Gid(value) => (13, *value),
        Auxv::Egid(value) => (14, *value),
        Auxv::Secure(value) => (23, *value),
        Auxv::Random(_) => (25, data_ptr.unwrap().as_isize() as usize),
    };

    push_usize_to_stack(sp, stack_bottom, value)?;
    push_usize_to_stack(sp, stack_bottom, auxv_type)?;
    Ok(())
}

pub(super) fn estimate_user_init_stack_size(
    argv: &[&[u8]],
    envp: &[&[u8]],
    auxv: &[Auxv],
) -> usize {
    let str_len = align_up(
        argv.iter().fold(0, |l, arg| l + arg.len() + 1)
            + envp.iter().fold(0, |l, env| l + env.len() + 1),
        size_of::<usize>(),
    );

    let aux_data_len = auxv.iter().fold(0, |l, aux| {
        l + match aux {
            Auxv::Random(_) => 16,
            _ => 0,
        }
    });

    let ptrs_len =
        (2 * (1 + auxv.len()) + argv.len() + 1 + envp.len() + 1 + 1) * size_of::<usize>();

    str_len + aux_data_len + ptrs_len
}

/// Initializes a user stack. See "Initial Process Stack" in <https://uclibc.org/docs/psABI-x86_64.pdf>.
pub(super) fn init_user_stack(
    user_stack_top: UserVAddr,
    stack_top: VAddr,
    stack_bottom: VAddr,
    argv: &[&[u8]],
    envp: &[&[u8]],
    auxv: &[Auxv],
) -> Result<UserVAddr> {
    let mut sp = stack_top;
    let kernel_sp_to_user_sp = |sp: VAddr| {
        let offset = stack_top.value() - sp.value();
        user_stack_top.sub(offset)
    };

    // Write auxv data (pushed in reverse so first entry is at highest address).
    // auxv_ptrs must match the forward order of auxv, so we reverse after building.
    let mut auxv_ptrs = Vec::with_capacity(auxv.len());
    for auxv in auxv.iter().rev() {
        push_aux_data_to_stack(&mut sp, stack_bottom, auxv)?;
        auxv_ptrs.push(Some(kernel_sp_to_user_sp(sp)));
    }
    auxv_ptrs.reverse();

    // Write envp strings.
    let mut envp_ptrs = Vec::with_capacity(envp.len());
    for env in envp {
        push_bytes_to_stack(&mut sp, stack_bottom, &[0])?;
        push_bytes_to_stack(&mut sp, stack_bottom, env)?;
        envp_ptrs.push(kernel_sp_to_user_sp(sp));
    }

    // Write argv strings.
    let mut argv_ptrs = Vec::with_capacity(argv.len());
    for arg in argv.iter().rev() {
        push_bytes_to_stack(&mut sp, stack_bottom, &[0])?;
        push_bytes_to_stack(&mut sp, stack_bottom, arg)?;
        argv_ptrs.push(kernel_sp_to_user_sp(sp));
    }

    // The length of the string table wrote above could be unaligned.
    sp = sp.align_down(size_of::<usize>());

    // Push auxiliary vector entries.
    push_auxv_entry_to_stack(&mut sp, stack_bottom, &Auxv::Null, None)?;
    for (aux, data) in auxv.iter().zip(auxv_ptrs.iter()) {
        push_auxv_entry_to_stack(&mut sp, stack_bottom, aux, *data)?;
    }

    // Push environment pointers (`const char **envp`).
    push_usize_to_stack(&mut sp, stack_bottom, 0)?;
    for ptr in envp_ptrs {
        push_usize_to_stack(&mut sp, stack_bottom, ptr.value())?;
    }

    // Push argument pointers (`const char **argv`).
    push_usize_to_stack(&mut sp, stack_bottom, 0)?;
    for ptr in argv_ptrs {
        push_usize_to_stack(&mut sp, stack_bottom, ptr.value())?;
    }

    // Push argc.
    push_usize_to_stack(&mut sp, stack_bottom, argv.len())?;

    Ok(kernel_sp_to_user_sp(sp))
}
