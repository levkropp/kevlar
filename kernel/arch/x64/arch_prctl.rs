// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::process::Process;
use crate::result::*;
use alloc::sync::Arc;
use kevlar_platform::arch::x64_specific::write_fsbase;

use kevlar_platform::address::UserVAddr;

const ARCH_SET_FS: i32 = 0x1002;

pub fn arch_prctl(current: &Arc<Process>, code: i32, uaddr: UserVAddr) -> Result<()> {
    match code {
        ARCH_SET_FS => {
            let value = uaddr.value() as u64;
            current.arch().fsbase.store(value);
            write_fsbase(value);
        }
        _ => {
            return Err(Errno::EINVAL.into());
        }
    }

    Ok(())
}
