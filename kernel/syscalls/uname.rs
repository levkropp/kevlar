// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::result::Result;
use crate::syscalls::SyscallHandler;
use kevlar_platform::address::UserVAddr;

/// The maximum length of a field in `struct utsname` including the trailing
/// null character.
const UTS_FIELD_LEN: usize = 65;

/// Pre-built utsname struct: build once on the stack, write once to userspace.
/// Layout: 6 fields × 65 bytes = 390 bytes.
fn build_utsname(buf: &mut [u8; 6 * UTS_FIELD_LEN]) {
    fn write_field(buf: &mut [u8; 6 * UTS_FIELD_LEN], idx: usize, value: &[u8]) {
        let offset = idx * UTS_FIELD_LEN;
        let len = value.len().min(UTS_FIELD_LEN - 1);
        buf[offset..offset + len].copy_from_slice(&value[..len]);
        // Rest is already zeroed
    }

    // sysname
    write_field(buf, 0, b"Linux");
    // nodename
    // (empty, already zeroed)
    // release — glibc checks kernel version for feature detection
    write_field(buf, 2, b"4.0.0");
    // version
    write_field(buf, 3, b"Kevlar");
    // machine — left empty (already zeroed)
    // domainname — left empty (already zeroed)
}

impl<'a> SyscallHandler<'a> {
    pub fn sys_uname(&mut self, buf: UserVAddr) -> Result<isize> {
        let mut utsname = [0u8; 6 * UTS_FIELD_LEN];
        build_utsname(&mut utsname);
        buf.write_bytes(&utsname)?;
        Ok(0)
    }
}
