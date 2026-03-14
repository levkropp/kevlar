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

        fn write_field(buf: &mut [u8; 6 * UTS_FIELD_LEN], idx: usize, value: &[u8]) {
            let offset = idx * UTS_FIELD_LEN;
            let len = value.len().min(UTS_FIELD_LEN - 1);
            buf[offset..offset + len].copy_from_slice(&value[..len]);
        }

        // sysname
        write_field(&mut utsname, 0, b"Linux");
        // nodename — from UTS namespace
        let ns = current_process().namespaces();
        let hostname = ns.uts.get_hostname();
        write_field(&mut utsname, 1, &hostname);
        // release — glibc checks kernel version for feature detection
        write_field(&mut utsname, 2, b"4.0.0");
        // version
        write_field(&mut utsname, 3, b"Kevlar");
        // machine
        #[cfg(target_arch = "x86_64")]
        write_field(&mut utsname, 4, b"x86_64");
        #[cfg(target_arch = "aarch64")]
        write_field(&mut utsname, 4, b"aarch64");
        // domainname — from UTS namespace
        let domainname = ns.uts.get_domainname();
        write_field(&mut utsname, 5, &domainname);

        buf.write_bytes(&utsname)?;
        Ok(0)
    }
}
