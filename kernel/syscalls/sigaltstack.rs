// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// Provenance: Own (POSIX sigaltstack(2) man page).
use core::sync::atomic::Ordering;

use crate::prelude::*;
use crate::process::current_process;
use crate::syscalls::SyscallHandler;
use kevlar_platform::address::UserVAddr;

/// Linux struct stack_t layout (x86_64):
///   [0]:  ss_sp    (8 bytes, void *)
///   [8]:  ss_flags (4 bytes, int)
///   [16]: ss_size  (8 bytes, size_t)
/// Total: 24 bytes (with padding).
const SS_DISABLE: u32 = 2;
const MINSIGSTKSZ: usize = 2048;

impl<'a> SyscallHandler<'a> {
    pub fn sys_sigaltstack(&mut self, ss: usize, old_ss: usize) -> Result<isize> {
        let proc = current_process();

        // Write current alt stack to old_ss if non-NULL.
        if let Some(old_ptr) = UserVAddr::new(old_ss) {
            let sp = proc.alt_stack_sp.load(Ordering::Relaxed);
            let flags = proc.alt_stack_flags.load(Ordering::Relaxed);
            let size = proc.alt_stack_size.load(Ordering::Relaxed);

            // Write struct stack_t { ss_sp, ss_flags, ss_size }.
            let mut buf = [0u8; 24];
            buf[0..8].copy_from_slice(&(sp as u64).to_ne_bytes());
            buf[8..12].copy_from_slice(&(flags as i32).to_ne_bytes());
            // Padding at [12..16] stays zero.
            buf[16..24].copy_from_slice(&(size as u64).to_ne_bytes());
            old_ptr.write_bytes(&buf)?;
        }

        // Read and apply new alt stack from ss if non-NULL.
        if let Some(ss_ptr) = UserVAddr::new(ss) {
            let buf = ss_ptr.read::<[u8; 24]>()?;
            let new_sp = u64::from_ne_bytes(buf[0..8].try_into().unwrap()) as usize;
            let new_flags = i32::from_ne_bytes(buf[8..12].try_into().unwrap()) as u32;
            let new_size = u64::from_ne_bytes(buf[16..24].try_into().unwrap()) as usize;

            if new_flags & SS_DISABLE != 0 {
                // Disable alt stack.
                proc.alt_stack_sp.store(0, Ordering::Relaxed);
                proc.alt_stack_size.store(0, Ordering::Relaxed);
                proc.alt_stack_flags.store(SS_DISABLE, Ordering::Relaxed);
            } else {
                if new_size < MINSIGSTKSZ {
                    return Err(Errno::ENOMEM.into());
                }
                proc.alt_stack_sp.store(new_sp, Ordering::Relaxed);
                proc.alt_stack_size.store(new_size, Ordering::Relaxed);
                proc.alt_stack_flags.store(0, Ordering::Relaxed);
            }
        }

        Ok(0)
    }
}
