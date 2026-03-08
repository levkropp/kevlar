// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Reference: OSv core/rlimit.cc (BSD-3-Clause) — getrlimit.
// Returns fake but reasonable limits: RLIM_INFINITY for most resources,
// 8 MB for stack, 1024 for NOFILE.
use crate::ctypes::c_int;
use crate::prelude::*;
use crate::syscalls::SyscallHandler;
use crate::user_buffer::UserBufWriter;
use core::mem::size_of;
use kevlar_runtime::address::UserVAddr;

const RLIM_INFINITY: u64 = !0u64;

const RLIMIT_STACK: c_int = 3;
const RLIMIT_NOFILE: c_int = 7;

#[repr(C)]
struct Rlimit {
    rlim_cur: u64,
    rlim_max: u64,
}

impl<'a> SyscallHandler<'a> {
    pub fn sys_getrlimit(&mut self, resource: c_int, buf: UserVAddr) -> Result<isize> {
        let limit = match resource {
            RLIMIT_STACK => Rlimit {
                rlim_cur: 8 * 1024 * 1024,  // 8 MB
                rlim_max: RLIM_INFINITY,
            },
            RLIMIT_NOFILE => Rlimit {
                rlim_cur: 1024,
                rlim_max: 1024,
            },
            _ => Rlimit {
                rlim_cur: RLIM_INFINITY,
                rlim_max: RLIM_INFINITY,
            },
        };

        let mut writer = UserBufWriter::from_uaddr(buf, 2 * size_of::<u64>());
        writer.write::<u64>(limit.rlim_cur)?;
        writer.write::<u64>(limit.rlim_max)?;
        Ok(0)
    }
}
