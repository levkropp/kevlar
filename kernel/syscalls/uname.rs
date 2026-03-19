// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::process::current_process;
use crate::result::Result;
use crate::syscalls::SyscallHandler;
use kevlar_platform::address::UserVAddr;

/// The maximum length of a field in `struct utsname` including the trailing
/// null character.
const UTS_FIELD_LEN: usize = 65;

impl<'a> SyscallHandler<'a> {
    pub fn sys_uname(&mut self, buf: UserVAddr) -> Result<isize> {
        let mut utsname = [0u8; 6 * UTS_FIELD_LEN];

        #[inline(always)]
        fn write_field(buf: &mut [u8; 6 * UTS_FIELD_LEN], idx: usize, value: &[u8]) {
            let offset = idx * UTS_FIELD_LEN;
            let len = value.len().min(UTS_FIELD_LEN - 1);
            buf[offset..offset + len].copy_from_slice(&value[..len]);
        }

        write_field(&mut utsname, 0, b"Linux");    // sysname
        write_field(&mut utsname, 2, b"6.19.8");   // release
        write_field(&mut utsname, 3, b"Kevlar");   // version
        #[cfg(target_arch = "x86_64")]
        write_field(&mut utsname, 4, b"x86_64");   // machine
        #[cfg(target_arch = "aarch64")]
        write_field(&mut utsname, 4, b"aarch64");

        // nodename + domainname from UTS namespace — read directly
        // from the lock without heap allocation.
        let proc = current_process();
        let uts_arc = proc.uts_namespace();
        let uts = &*uts_arc;
        uts.write_hostname_into(&mut utsname, 1);
        uts.write_domainname_into(&mut utsname, 5);

        buf.write_bytes(&utsname)?;
        Ok(0)
    }
}
