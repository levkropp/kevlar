// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::process::Process;
use crate::result::*;
use alloc::sync::Arc;
use kevlar_platform::arch::x64_specific::write_fsbase;

use kevlar_platform::address::UserVAddr;

const ARCH_SET_FS: i32 = 0x1002;
const ARCH_GET_FS: i32 = 0x1003;
const ARCH_SET_GS: i32 = 0x1001;
const ARCH_GET_GS: i32 = 0x1004;

pub fn arch_prctl(current: &Arc<Process>, code: i32, uaddr: UserVAddr) -> Result<()> {
    match code {
        ARCH_SET_FS => {
            let value = uaddr.value() as u64;
            current.arch().fsbase.store(value);
            write_fsbase(value);
        }
        ARCH_GET_FS => {
            let value = current.arch().fsbase.load();
            uaddr.write::<u64>(&value)?;
        }
        ARCH_SET_GS | ARCH_GET_GS => {
            // GS base: stub — not used by glibc TLS, return success.
        }
        _ => {
            return Err(Errno::EINVAL.into());
        }
    }

    Ok(())
}
